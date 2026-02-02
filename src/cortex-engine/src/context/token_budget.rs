//! Token budget management for context window optimization.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Token budget manager.
#[derive(Debug, Clone)]
pub struct TokenBudgetManager {
    /// Maximum tokens allowed.
    max_tokens: u32,
    /// Tokens reserved for output.
    output_reserve: u32,
    /// Current allocations by category.
    allocations: HashMap<String, TokenAllocation>,
    /// Budget strategy.
    strategy: BudgetStrategy,
}

impl TokenBudgetManager {
    /// Create a new token budget manager.
    pub fn new(max_tokens: u32, output_reserve: u32) -> Self {
        Self {
            max_tokens,
            output_reserve,
            allocations: HashMap::new(),
            strategy: BudgetStrategy::default(),
        }
    }

    /// Get maximum tokens.
    pub fn max_tokens(&self) -> u32 {
        self.max_tokens
    }

    /// Get available tokens for input.
    pub fn available_input_tokens(&self) -> u32 {
        self.max_tokens.saturating_sub(self.output_reserve)
    }

    /// Get current usage ratio (0.0 - 1.0).
    pub fn current_usage(&self) -> f32 {
        let used: u32 = self.allocations.values().map(|a| a.used).sum();
        used as f32 / self.available_input_tokens() as f32
    }

    /// Allocate tokens for a category.
    pub fn allocate(&mut self, category: impl Into<String>, tokens: u32) -> AllocationResult {
        let category = category.into();
        let available = self.remaining_tokens();

        if tokens > available {
            return AllocationResult::Partial {
                requested: tokens,
                allocated: available,
            };
        }

        let allocation = self
            .allocations
            .entry(category.clone())
            .or_insert(TokenAllocation {
                category,
                budget: 0,
                used: 0,
                priority: Priority::Normal,
            });

        allocation.used += tokens;
        AllocationResult::Full { allocated: tokens }
    }

    /// Set budget for a category.
    pub fn set_budget(&mut self, category: impl Into<String>, budget: u32, priority: Priority) {
        let category = category.into();
        self.allocations.insert(
            category.clone(),
            TokenAllocation {
                category,
                budget,
                used: 0,
                priority,
            },
        );
    }

    /// Get remaining tokens.
    pub fn remaining_tokens(&self) -> u32 {
        let used: u32 = self.allocations.values().map(|a| a.used).sum();
        self.available_input_tokens().saturating_sub(used)
    }

    /// Get usage for a category.
    pub fn get_usage(&self, category: &str) -> Option<u32> {
        self.allocations.get(category).map(|a| a.used)
    }

    /// Get budget for a category.
    pub fn get_budget(&self, category: &str) -> Option<u32> {
        self.allocations.get(category).map(|a| a.budget)
    }

    /// Clear allocations.
    pub fn clear(&mut self) {
        self.allocations.clear();
    }

    /// Reset usage for a category.
    pub fn reset_usage(&mut self, category: &str) {
        if let Some(alloc) = self.allocations.get_mut(category) {
            alloc.used = 0;
        }
    }

    /// Get all allocations.
    pub fn allocations(&self) -> &HashMap<String, TokenAllocation> {
        &self.allocations
    }

    /// Get budget report.
    pub fn report(&self) -> BudgetReport {
        let used: u32 = self.allocations.values().map(|a| a.used).sum();
        let available = self.available_input_tokens();

        BudgetReport {
            max_tokens: self.max_tokens,
            output_reserve: self.output_reserve,
            available_input: available,
            used_tokens: used,
            remaining_tokens: available.saturating_sub(used),
            usage_percent: (used as f32 / available as f32) * 100.0,
            allocations: self.allocations.values().cloned().collect(),
        }
    }

    /// Optimize allocations based on strategy.
    pub fn optimize(&mut self) {
        match self.strategy {
            BudgetStrategy::Fixed => {}
            BudgetStrategy::Proportional => self.optimize_proportional(),
            BudgetStrategy::Priority => self.optimize_priority(),
            BudgetStrategy::Dynamic => self.optimize_dynamic(),
        }
    }

    fn optimize_proportional(&mut self) {
        let total_budget: u32 = self.allocations.values().map(|a| a.budget).sum();
        let available = self.available_input_tokens();

        if total_budget == 0 {
            return;
        }

        for alloc in self.allocations.values_mut() {
            let ratio = alloc.budget as f32 / total_budget as f32;
            alloc.budget = (available as f32 * ratio) as u32;
        }
    }

    fn optimize_priority(&mut self) {
        let mut available = self.available_input_tokens();
        let mut sorted: Vec<_> = self.allocations.values_mut().collect();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.priority as u8));

        for alloc in sorted {
            let grant = alloc.budget.min(available);
            alloc.budget = grant;
            available = available.saturating_sub(grant);
        }
    }

    fn optimize_dynamic(&mut self) {
        // First pass: priority-based
        self.optimize_priority();

        // Second pass: redistribute unused budget
        let used: u32 = self.allocations.values().map(|a| a.used).sum();
        let remaining = self.available_input_tokens().saturating_sub(used);

        if remaining > 0 {
            let count = self.allocations.len() as u32;
            if let Some(extra_each) = remaining.checked_div(count) {
                for alloc in self.allocations.values_mut() {
                    alloc.budget += extra_each;
                }
            }
        }
    }

    /// Set strategy.
    pub fn set_strategy(&mut self, strategy: BudgetStrategy) {
        self.strategy = strategy;
    }
}

/// Token allocation for a category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenAllocation {
    /// Category name.
    pub category: String,
    /// Budgeted tokens.
    pub budget: u32,
    /// Currently used tokens.
    pub used: u32,
    /// Priority level.
    pub priority: Priority,
}

impl TokenAllocation {
    /// Get remaining budget.
    pub fn remaining(&self) -> u32 {
        self.budget.saturating_sub(self.used)
    }

    /// Check if over budget.
    pub fn is_over_budget(&self) -> bool {
        self.used > self.budget
    }

    /// Get usage ratio.
    pub fn usage_ratio(&self) -> f32 {
        if self.budget == 0 {
            return 0.0;
        }
        self.used as f32 / self.budget as f32
    }
}

/// Priority level for token allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Priority {
    /// Critical - always allocated first.
    Critical = 4,
    /// High priority.
    High = 3,
    /// Normal priority.
    #[default]
    Normal = 2,
    /// Low priority.
    Low = 1,
    /// Background - allocated last.
    Background = 0,
}

/// Allocation result.
#[derive(Debug, Clone)]
pub enum AllocationResult {
    /// Full allocation granted.
    Full { allocated: u32 },
    /// Partial allocation granted.
    Partial { requested: u32, allocated: u32 },
    /// No allocation possible.
    None { requested: u32 },
}

impl AllocationResult {
    /// Get allocated tokens.
    pub fn allocated(&self) -> u32 {
        match self {
            Self::Full { allocated } => *allocated,
            Self::Partial { allocated, .. } => *allocated,
            Self::None { .. } => 0,
        }
    }

    /// Check if full allocation.
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full { .. })
    }

    /// Check if any allocation.
    pub fn is_some(&self) -> bool {
        self.allocated() > 0
    }
}

/// Budget strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetStrategy {
    /// Fixed budgets per category.
    Fixed,
    /// Proportional to requested budgets.
    #[default]
    Proportional,
    /// Priority-based allocation.
    Priority,
    /// Dynamic reallocation.
    Dynamic,
}

/// Budget report.
#[derive(Debug, Clone, Serialize)]
pub struct BudgetReport {
    /// Maximum tokens.
    pub max_tokens: u32,
    /// Output reserve.
    pub output_reserve: u32,
    /// Available for input.
    pub available_input: u32,
    /// Used tokens.
    pub used_tokens: u32,
    /// Remaining tokens.
    pub remaining_tokens: u32,
    /// Usage percentage.
    pub usage_percent: f32,
    /// Per-category allocations.
    pub allocations: Vec<TokenAllocation>,
}

/// Token budget for a specific context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    /// System prompt budget.
    pub system: u32,
    /// Conversation history budget.
    pub history: u32,
    /// File context budget.
    pub files: u32,
    /// Tool definitions budget.
    pub tools: u32,
    /// Current message budget.
    pub current: u32,
    /// Reserved for response.
    pub response: u32,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self::for_model("gpt-4o")
    }
}

impl TokenBudget {
    /// Create budget for a specific model.
    pub fn for_model(model: &str) -> Self {
        let (total, response) = match model {
            m if m.starts_with("gpt-4o") => (128000, 16384),
            m if m.starts_with("gpt-4") => (128000, 8192),
            m if m.starts_with("o1") || m.starts_with("o3") => (200000, 100000),
            m if m.starts_with("claude-3") => (200000, 8192),
            m if m.starts_with("claude-2") => (100000, 4096),
            _ => (128000, 16384),
        };

        let available = total - response;

        Self {
            system: (available as f32 * 0.1) as u32,  // 10%
            history: (available as f32 * 0.5) as u32, // 50%
            files: (available as f32 * 0.25) as u32,  // 25%
            tools: (available as f32 * 0.05) as u32,  // 5%
            current: (available as f32 * 0.1) as u32, // 10%
            response,
        }
    }

    /// Get total budget.
    pub fn total(&self) -> u32 {
        self.system + self.history + self.files + self.tools + self.current + self.response
    }

    /// Get available input budget.
    pub fn available_input(&self) -> u32 {
        self.system + self.history + self.files + self.tools + self.current
    }

    /// Scale all budgets by a factor.
    pub fn scale(&mut self, factor: f32) {
        self.system = (self.system as f32 * factor) as u32;
        self.history = (self.history as f32 * factor) as u32;
        self.files = (self.files as f32 * factor) as u32;
        self.tools = (self.tools as f32 * factor) as u32;
        self.current = (self.current as f32 * factor) as u32;
    }

    /// Redistribute based on usage.
    pub fn redistribute(&mut self, usage: &TokenBudgetUsage) {
        let _total_available = self.available_input();

        // Calculate actual needs
        let _needed_history = usage.history.min(self.history * 2);
        let _needed_files = usage.files.min(self.files * 2);

        // If files need more and history needs less, reallocate
        if usage.files > self.files && usage.history < self.history {
            let surplus = self.history - usage.history;
            let transfer = surplus.min(usage.files - self.files);
            self.history -= transfer;
            self.files += transfer;
        }

        // Vice versa
        if usage.history > self.history && usage.files < self.files {
            let surplus = self.files - usage.files;
            let transfer = surplus.min(usage.history - self.history);
            self.files -= transfer;
            self.history += transfer;
        }
    }
}

/// Token budget usage tracking.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TokenBudgetUsage {
    /// System prompt usage.
    pub system: u32,
    /// History usage.
    pub history: u32,
    /// Files usage.
    pub files: u32,
    /// Tools usage.
    pub tools: u32,
    /// Current message usage.
    pub current: u32,
}

impl TokenBudgetUsage {
    /// Get total usage.
    pub fn total(&self) -> u32 {
        self.system + self.history + self.files + self.tools + self.current
    }

    /// Check if within budget.
    pub fn within_budget(&self, budget: &TokenBudget) -> bool {
        self.system <= budget.system
            && self.history <= budget.history
            && self.files <= budget.files
            && self.tools <= budget.tools
            && self.current <= budget.current
    }

    /// Get over-budget amounts.
    pub fn over_budget(&self, budget: &TokenBudget) -> TokenBudgetUsage {
        TokenBudgetUsage {
            system: self.system.saturating_sub(budget.system),
            history: self.history.saturating_sub(budget.history),
            files: self.files.saturating_sub(budget.files),
            tools: self.tools.saturating_sub(budget.tools),
            current: self.current.saturating_sub(budget.current),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_budget_manager() {
        let mut manager = TokenBudgetManager::new(128000, 16384);

        manager.set_budget("system", 10000, Priority::Critical);
        manager.set_budget("history", 50000, Priority::High);

        let result = manager.allocate("history", 1000);
        assert!(result.is_full());

        assert_eq!(manager.get_usage("history"), Some(1000));
    }

    #[test]
    fn test_allocation_partial() {
        let mut manager = TokenBudgetManager::new(1000, 100);

        // Try to allocate more than available
        let result = manager.allocate("test", 2000);

        match result {
            AllocationResult::Partial { allocated, .. } => {
                assert!(allocated <= 900); // 1000 - 100 reserve
            }
            _ => panic!("Expected partial allocation"),
        }
    }

    #[test]
    fn test_budget_report() {
        let mut manager = TokenBudgetManager::new(128000, 16384);
        manager.set_budget("system", 10000, Priority::Critical);
        manager.allocate("system", 5000);

        let report = manager.report();
        assert_eq!(report.used_tokens, 5000);
        assert!(report.usage_percent > 0.0);
    }

    #[test]
    fn test_token_budget_for_model() {
        let budget = TokenBudget::for_model("gpt-4o");
        assert_eq!(budget.response, 16384);

        let budget = TokenBudget::for_model("o1");
        assert_eq!(budget.response, 100000);
    }

    #[test]
    fn test_priority_allocation() {
        let mut manager = TokenBudgetManager::new(10000, 1000);
        manager.set_strategy(BudgetStrategy::Priority);

        manager.set_budget("critical", 5000, Priority::Critical);
        manager.set_budget("low", 5000, Priority::Low);

        manager.optimize();

        let critical_budget = manager.get_budget("critical").unwrap();
        let low_budget = manager.get_budget("low").unwrap();

        // Critical should get more
        assert!(critical_budget >= low_budget);
    }
}
