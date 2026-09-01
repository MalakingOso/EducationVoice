//! Inline SVG icon components, Phosphor regular weight.
//!
//! Every glyph is one 256-unit path drawn with `fill: currentColor`, so a
//! single component is ink on a plain row and the accent colour on the
//! selected nav tab without a second rule anywhere. Inline geometry also
//! cannot half-load the way a fetched asset can.

use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct IconProps {
    #[props(default = 20)]
    pub size: u32,
    #[props(default = String::new())]
    pub class: String,
}

// ─── The mark ────────────────────────────────────────────────────────────────

/// The app's symbol: the six rounded squares from `assets/icon.png`, without
/// the charcoal tile they sit on there.
///
/// The one exception to the single-path rule above, and a deliberate one. The
/// PNG draws the symbol for a dark ground, so its cap is off-white and one of
/// its outlines is white; lifted onto the light page those vanish. Each shape
/// is therefore its own element with its own class, coloured in styles.css
/// under `.mark-*`, where the fills can take the icon's own greys and the cap
/// the surface colour with an outline that survives the ground. The geometry
/// is the PNG's, sampled at 512, which is why the view box is not 256.
///
/// Draw order is the PNG's stacking, back to front: the bottom-left outline,
/// the two grey fills, the top-right outline, the orange square, the cap.
#[component]
pub fn IconMark(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 512 512",
            rect { class: "mark-line", x: "80", y: "304", width: "148", height: "148", rx: "24" }
            rect { class: "mark-fill", x: "164", y: "282", width: "100", height: "100", rx: "20" }
            rect { class: "mark-fill-light", x: "190", y: "180", width: "132", height: "132", rx: "28" }
            rect { class: "mark-line-thin", x: "295", y: "93", width: "110", height: "108", rx: "22" }
            rect { class: "mark-orange", x: "292", y: "180", width: "50", height: "50", rx: "10" }
            rect { class: "mark-cap", x: "357", y: "46", width: "66", height: "68", rx: "14" }
        }
    }
}

// ─── Navigation ──────────────────────────────────────────────────────────────

/// Nav entry for the Run page.
#[component]
pub fn IconWaveform(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M56,96v64a8,8,0,0,1-16,0V96a8,8,0,0,1,16,0ZM88,24a8,8,0,0,0-8,8V224a8,8,0,0,0,16,0V32A8,8,0,0,0,88,24Zm40,32a8,8,0,0,0-8,8V192a8,8,0,0,0,16,0V64A8,8,0,0,0,128,56Zm40,32a8,8,0,0,0-8,8v64a8,8,0,0,0,16,0V96A8,8,0,0,0,168,88Zm40-16a8,8,0,0,0-8,8v96a8,8,0,0,0,16,0V80A8,8,0,0,0,208,72Z"
            }
        }
    }
}

/// Nav entry for the Library page.
#[component]
pub fn IconListBullets(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M80,64a8,8,0,0,1,8-8H216a8,8,0,0,1,0,16H88A8,8,0,0,1,80,64Zm136,56H88a8,8,0,0,0,0,16H216a8,8,0,0,0,0-16Zm0,64H88a8,8,0,0,0,0,16H216a8,8,0,0,0,0-16ZM44,52A12,12,0,1,0,56,64,12,12,0,0,0,44,52Zm0,64a12,12,0,1,0,12,12A12,12,0,0,0,44,116Zm0,64a12,12,0,1,0,12,12A12,12,0,0,0,44,180Z"
            }
        }
    }
}

/// Nav entry for settings.
#[component]
pub fn IconGear(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M128,80a48,48,0,1,0,48,48A48.05,48.05,0,0,0,128,80Zm0,80a32,32,0,1,1,32-32A32,32,0,0,1,128,160Zm88-29.84q.06-2.16,0-4.32l14.92-18.64a8,8,0,0,0,1.48-7.06,107.21,107.21,0,0,0-10.88-26.25,8,8,0,0,0-6-3.93l-23.72-2.64q-1.48-1.56-3-3L186,40.54a8,8,0,0,0-3.94-6,107.71,107.71,0,0,0-26.25-10.87,8,8,0,0,0-7.06,1.49L130.16,40Q128,40,125.84,40L107.2,25.11a8,8,0,0,0-7.06-1.48A107.6,107.6,0,0,0,73.89,34.51a8,8,0,0,0-3.93,6L67.32,64.27q-1.56,1.49-3,3L40.54,70a8,8,0,0,0-6,3.94,107.71,107.71,0,0,0-10.87,26.25,8,8,0,0,0,1.49,7.06L40,125.84Q40,128,40,130.16L25.11,148.8a8,8,0,0,0-1.48,7.06,107.21,107.21,0,0,0,10.88,26.25,8,8,0,0,0,6,3.93l23.72,2.64q1.49,1.56,3,3L70,215.46a8,8,0,0,0,3.94,6,107.71,107.71,0,0,0,26.25,10.87,8,8,0,0,0,7.06-1.49L125.84,216q2.16.06,4.32,0l18.64,14.92a8,8,0,0,0,7.06,1.48,107.21,107.21,0,0,0,26.25-10.88,8,8,0,0,0,3.93-6l2.64-23.72q1.56-1.48,3-3L215.46,186a8,8,0,0,0,6-3.94,107.71,107.71,0,0,0,10.87-26.25,8,8,0,0,0-1.49-7.06Zm-16.1-6.5a73.93,73.93,0,0,1,0,8.68,8,8,0,0,0,1.74,5.48l14.19,17.73a91.57,91.57,0,0,1-6.23,15L187,173.11a8,8,0,0,0-5.1,2.64,74.11,74.11,0,0,1-6.14,6.14,8,8,0,0,0-2.64,5.1l-2.51,22.58a91.32,91.32,0,0,1-15,6.23l-17.74-14.19a8,8,0,0,0-5-1.75h-.48a73.93,73.93,0,0,1-8.68,0,8,8,0,0,0-5.48,1.74L100.45,215.8a91.57,91.57,0,0,1-15-6.23L82.89,187a8,8,0,0,0-2.64-5.1,74.11,74.11,0,0,1-6.14-6.14,8,8,0,0,0-5.1-2.64L46.43,170.6a91.32,91.32,0,0,1-6.23-15l14.19-17.74a8,8,0,0,0,1.74-5.48,73.93,73.93,0,0,1,0-8.68,8,8,0,0,0-1.74-5.48L40.2,100.45a91.57,91.57,0,0,1,6.23-15L69,82.89a8,8,0,0,0,5.1-2.64,74.11,74.11,0,0,1,6.14-6.14A8,8,0,0,0,82.89,69L85.4,46.43a91.32,91.32,0,0,1,15-6.23l17.74,14.19a8,8,0,0,0,5.48,1.74,73.93,73.93,0,0,1,8.68,0,8,8,0,0,0,5.48-1.74L155.55,40.2a91.57,91.57,0,0,1,15,6.23L173.11,69a8,8,0,0,0,2.64,5.1,74.11,74.11,0,0,1,6.14,6.14,8,8,0,0,0,5.1,2.64l22.58,2.51a91.32,91.32,0,0,1,6.23,15l-14.19,17.74A8,8,0,0,0,199.87,123.66Z"
            }
        }
    }
}

// ─── Run and playback ────────────────────────────────────────────────────────

/// Play an episode.
#[component]
pub fn IconPlay(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M232.4,114.49,88.32,26.35a16,16,0,0,0-16.2-.3A15.86,15.86,0,0,0,64,39.87V216.13A15.94,15.94,0,0,0,80,232a16.07,16.07,0,0,0,8.36-2.35L232.4,141.51a15.81,15.81,0,0,0,0-27ZM80,215.94V40l143.83,88Z"
            }
        }
    }
}

/// Cancel a run that is under way.
#[component]
pub fn IconStop(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M200,40H56A16,16,0,0,0,40,56V200a16,16,0,0,0,16,16H200a16,16,0,0,0,16-16V56A16,16,0,0,0,200,40Zm0,160H56V56H200V200Z"
            }
        }
    }
}

/// Re-voice an existing script.
#[component]
pub fn IconArrowClockwise(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M240,56v48a8,8,0,0,1-8,8H184a8,8,0,0,1,0-16H211.4L184.81,71.64l-.25-.24a80,80,0,1,0-1.67,114.78,8,8,0,0,1,11,11.63A95.44,95.44,0,0,1,128,224h-1.32A96,96,0,1,1,195.75,60L224,85.8V56a8,8,0,1,1,16,0Z"
            }
        }
    }
}

/// Open the output folder in the file manager.
#[component]
pub fn IconFolderOpen(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M245,110.64A16,16,0,0,0,232,104H216V88a16,16,0,0,0-16-16H130.67L102.94,51.2a16.14,16.14,0,0,0-9.6-3.2H40A16,16,0,0,0,24,64V208h0a8,8,0,0,0,8,8H211.1a8,8,0,0,0,7.59-5.47l28.49-85.47A16.05,16.05,0,0,0,245,110.64ZM93.34,64,123.2,86.4A8,8,0,0,0,128,88h72v16H69.77a16,16,0,0,0-15.18,10.94L40,158.7V64Zm112,136H43.1l26.67-80H232Z"
            }
        }
    }
}

// ─── Window controls ─────────────────────────────────────────────────────────

/// The drop target's glyph: an arrow into a tray.
///
/// Stroked rather than filled, so it can be drawn from two short paths; the
/// 16-unit stroke with round caps and joins is what makes it sit with the
/// Phosphor glyphs around it, which are built on the same weight.
#[component]
pub fn IconTrayArrowDown(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "16",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M40,144V200a16,16,0,0,0,16,16H200a16,16,0,0,0,16-16V144" }
            path { d: "M128,32V152M88,112l40,40,40-40" }
        }
    }
}

/// Minimise.
#[component]
pub fn IconMinus(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M224,128a8,8,0,0,1-8,8H40a8,8,0,0,1,0-16H216A8,8,0,0,1,224,128Z"
            }
        }
    }
}

/// Close.
#[component]
pub fn IconX(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M205.66,194.34a8,8,0,0,1-11.32,11.32L128,139.31,61.66,205.66a8,8,0,0,1-11.32-11.32L116.69,128,50.34,61.66A8,8,0,0,1,61.66,50.34L128,116.69l66.34-66.35a8,8,0,0,1,11.32,11.32L139.31,128Z"
            }
        }
    }
}

/// Move a script turn earlier.
#[component]
pub fn IconCaretUp(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path { d: "M128,80 L176,160 L80,160 Z" }
        }
    }
}

/// Move a script turn later.
#[component]
pub fn IconCaretDown(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path { d: "M128,176 L80,96 L176,96 Z" }
        }
    }
}

/// Send an episode to Spotify. Phosphor's "Spotify Logo" brand glyph, used
/// to indicate interoperability on a "send to Spotify" action rather than
/// any endorsement — the same convention as any app's "Share to X" button
/// carrying X's mark.
#[component]
pub fn IconSpotify(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M128,24A104,104,0,1,0,232,128,104.11,104.11,0,0,0,128,24Zm0,192a88,88,0,1,1,88-88A88.1,88.1,0,0,1,128,216Zm31.07-46.26a8,8,0,0,1-10.81,3.33,42.79,42.79,0,0,0-40.52,0,8,8,0,0,1-7.48-14.14,59.33,59.33,0,0,1,55.48,0A8,8,0,0,1,159.07,169.74Zm32-56a8,8,0,0,1-10.83,3.29,110.62,110.62,0,0,0-104.46,0,8,8,0,0,1-7.54-14.12,126.67,126.67,0,0,1,119.54,0A8,8,0,0,1,191.06,113.76Zm-16,28a8,8,0,0,1-10.82,3.3,77,77,0,0,0-72.48,0,8,8,0,0,1-7.52-14.12,93,93,0,0,1,87.52,0A8,8,0,0,1,175.06,141.76Z"
            }
        }
    }
}

/// Open a saved script file in its default external app.
#[component]
pub fn IconFileText(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M213.66,82.34l-56-56A8,8,0,0,0,152,24H56A16,16,0,0,0,40,40V216a16,16,0,0,0,16,16H200a16,16,0,0,0,16-16V88A8,8,0,0,0,213.66,82.34ZM160,51.31,188.69,80H160ZM200,216H56V40h88V88a8,8,0,0,0,8,8h48V216ZM168,144a8,8,0,0,1-8,8H96a8,8,0,0,1,0-16h64A8,8,0,0,1,168,144Zm0,32a8,8,0,0,1-8,8H96a8,8,0,0,1,0-16h64A8,8,0,0,1,168,176Z"
            }
        }
    }
}

// ─── Utility ─────────────────────────────────────────────────────────────────

/// Confirmation — a resolved path, a finished step.
#[component]
pub fn IconCheck(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M229.66,77.66l-128,128a8,8,0,0,1-11.32,0l-56-56a8,8,0,0,1,11.32-11.32L96,188.69,218.34,66.34a8,8,0,0,1,11.32,11.32Z"
            }
        }
    }
}

/// Add a row.
#[component]
pub fn IconPlus(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M224,128a8,8,0,0,1-8,8H136v80a8,8,0,0,1-16,0V136H40a8,8,0,0,1,0-16h80V40a8,8,0,0,1,16,0v80h80A8,8,0,0,1,224,128Z"
            }
        }
    }
}

/// Copy to clipboard.
#[component]
pub fn IconCopy(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M216,32H88a8,8,0,0,0-8,8V80H40a8,8,0,0,0-8,8V216a8,8,0,0,0,8,8H168a8,8,0,0,0,8-8V176h40a8,8,0,0,0,8-8V40A8,8,0,0,0,216,32ZM160,208H48V96H160Zm48-48H176V88a8,8,0,0,0-8-8H96V48H208Z"
            }
        }
    }
}

/// Delete.
#[component]
pub fn IconTrash(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M216,48H176V40a24,24,0,0,0-24-24H104A24,24,0,0,0,80,40v8H40a8,8,0,0,0,0,16h8V208a16,16,0,0,0,16,16H192a16,16,0,0,0,16-16V64h8a8,8,0,0,0,0-16ZM96,40a8,8,0,0,1,8-8h48a8,8,0,0,1,8,8v8H96Zm96,168H64V64H192ZM112,104v64a8,8,0,0,1-16,0V104a8,8,0,0,1,16,0Zm48,0v64a8,8,0,0,1-16,0V104a8,8,0,0,1,16,0Z"
            }
        }
    }
}
