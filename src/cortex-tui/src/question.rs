//! Question prompt system for interactive user input.
//!
//! Provides a TUI for asking users questions with:
//! - Multiple choice selection (single/multi)
//! - Custom text input ("Type your own answer")
//! - Mouse hover and click support
//! - Keyboard navigation (↑↓, 1-9, Enter, Esc)
//! - Tab navigation for multiple questions

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================
// QUESTION TYPES
// ============================================================

/// A single option for a question
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub selected: bool,
}

/// Type of question (single choice, multiple choice, text, number)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionType {
    #[default]
    Single,
    Multiple,
    Text,
    Number,
}

/// A single question with its options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub question: String,
    #[serde(rename = "type", default)]
    pub question_type: QuestionType,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default = "default_true")]
    pub allow_custom: bool,
}

fn default_true() -> bool {
    true
}

/// A request for questions from the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionRequest {
    /// Unique ID for this request (tool call ID)
    pub id: String,
    /// Title for the questions form
    pub title: String,
    /// Description of why questions are being asked
    #[serde(default)]
    pub description: Option<String>,
    /// The questions to ask
    pub questions: Vec<Question>,
}

impl QuestionRequest {
    /// Parse a QuestionRequest from tool arguments
    pub fn from_tool_args(tool_call_id: &str, args: &Value) -> Option<Self> {
        let title = args.get("title")?.as_str()?.to_string();
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);

        let questions_arr = args.get("questions")?.as_array()?;
        let mut questions = Vec::new();

        for q in questions_arr {
            let id = q
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("q")
                .to_string();
            let question_text = q.get("question").and_then(|v| v.as_str())?.to_string();
            let q_type = match q.get("type").and_then(|v| v.as_str()).unwrap_or("single") {
                "multiple" => QuestionType::Multiple,
                "text" => QuestionType::Text,
                "number" => QuestionType::Number,
                _ => QuestionType::Single,
            };

            let options = q
                .get("options")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|opt| {
                            Some(QuestionOption {
                                value: opt.get("value").and_then(|v| v.as_str())?.to_string(),
                                label: opt.get("label").and_then(|v| v.as_str())?.to_string(),
                                description: opt
                                    .get("description")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                                selected: opt
                                    .get("selected")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            let placeholder = q
                .get("placeholder")
                .and_then(|v| v.as_str())
                .map(String::from);
            let required = q.get("required").and_then(|v| v.as_bool()).unwrap_or(true);
            let allow_custom = q
                .get("allow_custom")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            questions.push(Question {
                id,
                question: question_text,
                question_type: q_type,
                options,
                placeholder,
                required,
                allow_custom,
            });
        }

        Some(QuestionRequest {
            id: tool_call_id.to_string(),
            title,
            description,
            questions,
        })
    }
}

// ============================================================
// QUESTION STATE
// ============================================================

/// State for managing a question prompt session
#[derive(Debug, Clone)]
pub struct QuestionState {
    /// The request being answered
    pub request: QuestionRequest,
    /// Current tab/question index
    pub current_tab: usize,
    /// Selected option index per question
    pub selected_index: Vec<usize>,
    /// Answers for each question (selected labels)
    pub answers: Vec<Vec<String>>,
    /// Custom text input per question
    pub custom_input: Vec<String>,
    /// Whether we're editing custom input
    pub editing_custom: bool,
    /// Current custom input text (while editing)
    pub current_custom_text: String,
    /// Whether we're on the confirm tab
    pub on_confirm_tab: bool,
}

impl QuestionState {
    /// Create a new QuestionState from a request
    pub fn new(request: QuestionRequest) -> Self {
        let num_questions = request.questions.len();
        Self {
            request,
            current_tab: 0,
            selected_index: vec![0; num_questions],
            answers: vec![Vec::new(); num_questions],
            custom_input: vec![String::new(); num_questions],
            editing_custom: false,
            current_custom_text: String::new(),
            on_confirm_tab: false,
        }
    }

    /// Get the current question
    pub fn current_question(&self) -> Option<&Question> {
        self.request.questions.get(self.current_tab)
    }

    /// Is this a single question form (no tabs needed)?
    pub fn is_single_question(&self) -> bool {
        self.request.questions.len() == 1
            && self
                .current_question()
                .map(|q| q.question_type != QuestionType::Multiple)
                .unwrap_or(false)
    }

    /// Total number of tabs (questions + confirm if multiple)
    pub fn total_tabs(&self) -> usize {
        if self.is_single_question() {
            1
        } else {
            self.request.questions.len() + 1 // +1 for confirm tab
        }
    }

    /// Get the number of selectable options (including custom if allowed)
    pub fn option_count(&self) -> usize {
        if let Some(q) = self.current_question() {
            let base = q.options.len();
            if q.allow_custom { base + 1 } else { base }
        } else {
            0
        }
    }

    /// Is the current selection on "custom input"?
    pub fn is_on_custom(&self) -> bool {
        if let Some(q) = self.current_question() {
            q.allow_custom && self.selected_index[self.current_tab] == q.options.len()
        } else {
            false
        }
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.on_confirm_tab {
            return;
        }
        let count = self.option_count();
        if count > 0 {
            let idx = &mut self.selected_index[self.current_tab];
            *idx = (*idx + count - 1) % count;
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        if self.on_confirm_tab {
            return;
        }
        let count = self.option_count();
        if count > 0 {
            let idx = &mut self.selected_index[self.current_tab];
            *idx = (*idx + 1) % count;
        }
    }

    /// Move to specific option index
    pub fn move_to(&mut self, index: usize) {
        if self.on_confirm_tab {
            return;
        }
        let count = self.option_count();
        if index < count {
            self.selected_index[self.current_tab] = index;
        }
    }

    /// Move to next tab
    pub fn next_tab(&mut self) {
        let _total = self.total_tabs();
        if self.on_confirm_tab {
            self.on_confirm_tab = false;
            self.current_tab = 0;
        } else if self.current_tab + 1 < self.request.questions.len() {
            self.current_tab += 1;
        } else if !self.is_single_question() {
            self.on_confirm_tab = true;
        }
    }

    /// Move to previous tab
    pub fn prev_tab(&mut self) {
        if self.on_confirm_tab {
            self.on_confirm_tab = false;
            self.current_tab = self.request.questions.len().saturating_sub(1);
        } else if self.current_tab > 0 {
            self.current_tab -= 1;
        } else if !self.is_single_question() {
            self.on_confirm_tab = true;
        }
    }

    /// Select a specific tab
    pub fn select_tab(&mut self, index: usize) {
        if index < self.request.questions.len() {
            self.on_confirm_tab = false;
            self.current_tab = index;
        } else if index == self.request.questions.len() && !self.is_single_question() {
            self.on_confirm_tab = true;
        }
    }

    /// Toggle selection of current option (for multiple choice)
    pub fn toggle_current(&mut self) {
        if self.on_confirm_tab {
            return;
        }

        // Extract all needed data from current_question before mutating
        let (allow_custom, options_len, q_type, label) = {
            let Some(q) = self.current_question() else {
                return;
            };
            let selected_idx = self.selected_index[self.current_tab];
            let label = if selected_idx < q.options.len() {
                Some(q.options[selected_idx].label.clone())
            } else {
                None
            };
            (q.allow_custom, q.options.len(), q.question_type, label)
        };

        let selected_idx = self.selected_index[self.current_tab];

        if allow_custom && selected_idx == options_len {
            // Custom input selected - start editing
            self.editing_custom = true;
            self.current_custom_text = self.custom_input[self.current_tab].clone();
            return;
        }

        if selected_idx >= options_len {
            return;
        }

        let label = label.expect("label should exist for valid option index");
        let answers = &mut self.answers[self.current_tab];

        if q_type == QuestionType::Multiple {
            // Toggle in the list
            if let Some(pos) = answers.iter().position(|a| a == &label) {
                answers.remove(pos);
            } else {
                answers.push(label);
            }
        } else {
            // Single choice - replace and move to next
            answers.clear();
            answers.push(label);

            if self.is_single_question() {
                // Single question single choice - auto submit handled elsewhere
            } else {
                self.next_tab();
            }
        }
    }

    /// Confirm custom input
    pub fn confirm_custom_input(&mut self) {
        if !self.editing_custom {
            return;
        }

        let text = self.current_custom_text.trim().to_string();
        if !text.is_empty() {
            // Extract question type before mutating
            let q_type = self.current_question().map(|q| q.question_type);

            self.custom_input[self.current_tab] = text.clone();
            let answers = &mut self.answers[self.current_tab];

            if let Some(q_type) = q_type {
                if q_type == QuestionType::Multiple {
                    // Add to answers if not already there
                    if !answers.contains(&text) {
                        answers.push(text);
                    }
                } else {
                    // Replace answer
                    answers.clear();
                    answers.push(text);
                }
            }
        }

        self.editing_custom = false;
        self.current_custom_text.clear();

        // Move to next tab for single choice (after releasing mutable borrow)
        let _should_advance = self
            .current_question()
            .map(|q| q.question_type != QuestionType::Multiple)
            .unwrap_or(false)
            && !self.is_single_question()
            && !self.current_custom_text.is_empty();

        // Actually we already cleared current_custom_text, check the saved custom_input
        let text_was_saved = !self.custom_input[self.current_tab].is_empty();
        let q_type = self.current_question().map(|q| q.question_type);
        if text_was_saved && q_type == Some(QuestionType::Single) && !self.is_single_question() {
            self.next_tab();
        }
    }

    /// Cancel custom input editing
    pub fn cancel_custom_input(&mut self) {
        self.editing_custom = false;
        self.current_custom_text.clear();
    }

    /// Check if a specific answer is selected
    pub fn is_answer_selected(&self, tab: usize, answer: &str) -> bool {
        self.answers
            .get(tab)
            .map(|a| a.contains(&answer.to_string()))
            .unwrap_or(false)
    }

    /// Check if custom input is selected as an answer
    pub fn is_custom_selected(&self, tab: usize) -> bool {
        let custom = &self.custom_input[tab];
        !custom.is_empty() && self.is_answer_selected(tab, custom)
    }

    /// Get formatted answers for the LLM
    pub fn get_formatted_answers(&self) -> Value {
        let mut result = serde_json::Map::new();

        for (i, q) in self.request.questions.iter().enumerate() {
            let answer = &self.answers[i];
            let value = if q.question_type == QuestionType::Multiple {
                Value::Array(answer.iter().map(|a| Value::String(a.clone())).collect())
            } else {
                Value::String(answer.first().cloned().unwrap_or_default())
            };
            result.insert(q.id.clone(), value);
        }

        Value::Object(result)
    }

    /// Check if all required questions are answered
    pub fn is_complete(&self) -> bool {
        for (i, q) in self.request.questions.iter().enumerate() {
            if q.required && self.answers[i].is_empty() {
                return false;
            }
        }
        true
    }

    /// Get a short header for a question (for tabs)
    pub fn get_header(&self, index: usize) -> String {
        self.request
            .questions
            .get(index)
            .map(|q| {
                // Truncate to 12 chars max
                let text = &q.question;
                if text.len() > 12 {
                    format!("{}…", &text[..11])
                } else {
                    text.clone()
                }
            })
            .unwrap_or_else(|| format!("Q{}", index + 1))
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_question_request() {
        let args = json!({
            "title": "Test Questions",
            "description": "Some test questions",
            "questions": [
                {
                    "id": "q1",
                    "question": "What is your favorite color?",
                    "type": "single",
                    "options": [
                        {"value": "red", "label": "Red"},
                        {"value": "blue", "label": "Blue"}
                    ]
                }
            ]
        });

        let request = QuestionRequest::from_tool_args("test-id", &args).unwrap();
        assert_eq!(request.title, "Test Questions");
        assert_eq!(request.questions.len(), 1);
        assert_eq!(request.questions[0].options.len(), 2);
    }

    #[test]
    fn test_question_state_navigation() {
        let request = QuestionRequest {
            id: "test".to_string(),
            title: "Test".to_string(),
            description: None,
            questions: vec![Question {
                id: "q1".to_string(),
                question: "Q1".to_string(),
                question_type: QuestionType::Single,
                options: vec![
                    QuestionOption {
                        value: "a".to_string(),
                        label: "A".to_string(),
                        description: None,
                        selected: false,
                    },
                    QuestionOption {
                        value: "b".to_string(),
                        label: "B".to_string(),
                        description: None,
                        selected: false,
                    },
                ],
                placeholder: None,
                required: true,
                allow_custom: true,
            }],
        };

        let mut state = QuestionState::new(request);
        assert_eq!(state.selected_index[0], 0);

        state.move_down();
        assert_eq!(state.selected_index[0], 1);

        state.move_down();
        assert_eq!(state.selected_index[0], 2); // custom option

        state.move_down();
        assert_eq!(state.selected_index[0], 0); // wrap around
    }
}
