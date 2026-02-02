//! Builder for temperature selection.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};

struct TemperatureOption {
    value: f32,
    label: &'static str,
    description: &'static str,
}

const TEMPERATURES: &[TemperatureOption] = &[
    TemperatureOption {
        value: 0.0,
        label: "0.0",
        description: "Deterministic - always same output",
    },
    TemperatureOption {
        value: 0.3,
        label: "0.3",
        description: "Precise - minimal variation",
    },
    TemperatureOption {
        value: 0.5,
        label: "0.5",
        description: "Focused - slight creativity",
    },
    TemperatureOption {
        value: 0.7,
        label: "0.7",
        description: "Balanced - default",
    },
    TemperatureOption {
        value: 1.0,
        label: "1.0",
        description: "Creative - more variation",
    },
    TemperatureOption {
        value: 1.3,
        label: "1.3",
        description: "Experimental - high variation",
    },
    TemperatureOption {
        value: 1.5,
        label: "1.5",
        description: "Wild - very random",
    },
    TemperatureOption {
        value: 2.0,
        label: "2.0",
        description: "Maximum randomness",
    },
];

/// Build an interactive state for temperature selection.
pub fn build_temperature_selector(current: f32) -> InteractiveState {
    let items: Vec<InteractiveItem> = TEMPERATURES
        .iter()
        .map(|t| {
            let is_current = (t.value - current).abs() < 0.05;
            InteractiveItem::new(t.label, t.label)
                .with_description(t.description)
                .with_current(is_current)
                .with_icon(if is_current { '>' } else { ' ' })
        })
        .collect();

    InteractiveState::new(
        "Temperature",
        items,
        InteractiveAction::Custom("temperature".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_temperature_selector() {
        let state = build_temperature_selector(0.7);
        assert!(!state.items.is_empty());
        assert_eq!(state.title, "Temperature");

        // Check that 0.7 is marked as current
        let current = state.items.iter().find(|i| i.is_current);
        assert!(current.is_some());
        assert_eq!(current.unwrap().id, "0.7");
    }
}
