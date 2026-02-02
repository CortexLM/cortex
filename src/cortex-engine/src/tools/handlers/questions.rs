//! Questions tool handler.
//!
//! Presents interactive questions to gather requirements from the user.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ToolHandler;
use crate::error::Result;
use crate::tools::context::ToolContext;
use crate::tools::spec::{ToolMetadata, ToolResult};

/// Question option for single/multiple choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct QuestionOption {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Question type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum QuestionType {
    Single,
    Multiple,
    Text,
    Number,
}

/// A question to ask the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Question {
    pub id: String,
    pub question: String,
    #[serde(rename = "type")]
    pub question_type: QuestionType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<QuestionOption>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub allow_custom: bool,
}

/// Questions handler for gathering requirements.
pub struct QuestionsHandler;

impl QuestionsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for QuestionsHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for QuestionsHandler {
    fn name(&self) -> &str {
        "Questions"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let title = arguments
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Questions");

        let description = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let questions = arguments.get("questions").cloned().unwrap_or(json!([]));

        // Create questions data structure
        let questions_data = json!({
            "type": "questions",
            "title": title,
            "description": description,
            "questions": questions,
            "status": "pending_answers"
        });

        // Format output as readable text
        let mut output = format!("# {}\n\n", title);
        if !description.is_empty() {
            output.push_str(&format!("{}\n\n", description));
        }

        if let Some(q_arr) = questions.as_array() {
            for (i, q) in q_arr.iter().enumerate() {
                let question_text = q
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Question");
                let q_type = q.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                let required = q.get("required").and_then(|v| v.as_bool()).unwrap_or(false);

                output.push_str(&format!(
                    "{}. {}{}\n",
                    i + 1,
                    question_text,
                    if required { " *" } else { "" }
                ));

                // Show options for choice questions
                if let Some(options) = q.get("options").and_then(|v| v.as_array()) {
                    for opt in options {
                        let label = opt
                            .get("label")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Option");
                        let desc = opt.get("description").and_then(|v| v.as_str());

                        if q_type == "single" {
                            output.push_str(&format!("   ○ {}", label));
                        } else {
                            output.push_str(&format!("   □ {}", label));
                        }

                        if let Some(d) = desc {
                            output.push_str(&format!(" - {}", d));
                        }
                        output.push('\n');
                    }
                }

                // Show input hint for text/number
                if q_type == "text" || q_type == "number" {
                    let placeholder = q.get("placeholder").and_then(|v| v.as_str()).unwrap_or("");
                    if !placeholder.is_empty() {
                        output.push_str(&format!("   [{}]\n", placeholder));
                    }
                }

                output.push('\n');
            }
        }

        Ok(ToolResult::success(output).with_metadata(ToolMetadata {
            duration_ms: 0,
            exit_code: None,
            files_modified: vec![],
            data: Some(questions_data),
        }))
    }
}
