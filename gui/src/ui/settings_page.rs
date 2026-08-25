//! Settings: the knobs that outlive a single run.

use dioxus::prelude::*;

use crate::config::{save_config, Config};
use crate::ui::app::AppState;
use crate::ui::components::{Card, Select};

/// `ZE_AFFINITY_MASK` values. Empty means "do not set it", leaving the choice
/// to the Python side's own device detection.
const DEVICES: &[(&str, &str)] = &[
    ("", "Automatic"),
    ("0", "Arc B570 (10 GB)"),
    ("1", "Arc Pro B60 (22.7 GB)"),
];

#[component]
pub fn SettingsPage(state: AppState) -> Element {
    let mut state = state;
    let cfg = state.config.read().clone();
    let paths = state.paths.read().clone();

    rsx! {
        div { class: "content",
            Card { title: "Graphics",
                div { class: "card-row",
                    span { class: "card-label", "Device" }
                    Select {
                        value: cfg.device.gpu_mask.clone(),
                        options: DEVICES.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect(),
                        onchange: move |v: String| {
                            save_config(&mut state.config, |c| c.device.gpu_mask = v);
                        },
                    }
                }
                div { class: "card-label-hint",
                    "Selecting a card sets ZE_AFFINITY_MASK for the run. The B60 has the headroom; the B570 also drives the desktop."
                }
            }

            Card { title: "Where things are",
                if let Some(p) = paths {
                    div { class: "card-row",
                        span { class: "card-label", "Project" }
                        span { class: "debug-text", "{p.root.display()}" }
                    }
                    div { class: "card-row",
                        span { class: "card-label", "Interpreter" }
                        span { class: "debug-text", "{p.python.display()}" }
                    }
                    div { class: "card-row",
                        span { class: "card-label", "Episodes" }
                        span { class: "debug-text", "{p.output_dir.display()}" }
                    }
                }
                div { class: "card-row",
                    span { class: "card-label", "Settings" }
                    span { class: "debug-text", "{Config::config_path().display()}" }
                }
            }
        }
    }
}
