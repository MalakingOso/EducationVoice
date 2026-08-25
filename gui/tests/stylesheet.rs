//! Stylesheet invariants.
//!
//! Every failure guarded here is invisible: none of them errors, warns, or
//! fails to compile. A missing font silently falls back, an undefined class
//! silently renders wrong, and an escaped `--phos` renders green-on-cream at
//! 1.4:1 — technically drawn, practically unreadable.

const CSS: &str = include_str!("../assets/styles.css");

/// Every component source that carries markup. Listed explicitly rather than
/// walked, so adding a page without adding it here is a visible omission in
/// the diff rather than a silent gap in coverage.
const MARKUP: &[(&str, &str)] = &[
    ("app.rs", include_str!("../src/ui/app.rs")),
    ("components.rs", include_str!("../src/ui/components.rs")),
    ("run_strip.rs", include_str!("../src/ui/run_strip.rs")),
    ("run_page.rs", include_str!("../src/ui/run_page.rs")),
    ("script_page.rs", include_str!("../src/ui/script_page.rs")),
    ("library_page.rs", include_str!("../src/ui/library_page.rs")),
    ("settings_page.rs", include_str!("../src/ui/settings_page.rs")),
];

/// Families the browser resolves itself; naming one is never a bundling claim.
const GENERIC_FAMILIES: &[&str] = &[
    "monospace",
    "sans-serif",
    "serif",
    "system-ui",
    "cursive",
    "fantasy",
    "inherit",
    "ui-monospace",
    "-apple-system",
    "BlinkMacSystemFont",
];

/// Split the stylesheet into (selector, block) pairs at brace depth 1.
fn rules(css: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut selector = String::new();
    let mut block = String::new();
    let mut depth = 0usize;

    for ch in css.chars() {
        match ch {
            '{' => {
                depth += 1;
                if depth > 1 {
                    block.push(ch);
                }
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    out.push((selector.trim().to_string(), std::mem::take(&mut block)));
                    selector.clear();
                } else {
                    block.push(ch);
                }
            }
            _ => {
                if depth == 0 {
                    selector.push(ch);
                } else {
                    block.push(ch);
                }
            }
        }
    }
    out
}

#[test]
fn every_family_named_here_is_a_family_that_is_embedded() {
    // Collect what @font-face actually ships.
    let mut embedded: Vec<String> = Vec::new();
    for (selector, block) in rules(CSS) {
        if selector.trim_start().starts_with("@font-face") {
            for line in block.lines() {
                if let Some(v) = line.trim().strip_prefix("font-family:") {
                    embedded.push(v.trim().trim_end_matches(';').trim_matches('"').to_string());
                }
            }
        }
    }
    assert!(
        !embedded.is_empty(),
        "the stylesheet must bundle its faces with @font-face"
    );

    // Every family named in any font-family stack must be one of those, or generic.
    for (i, line) in CSS.lines().enumerate() {
        let trimmed = line.trim();
        let Some(value) = trimmed.strip_prefix("font-family:") else {
            continue;
        };
        for family in value.trim_end_matches(';').split(',') {
            let name = family.trim().trim_matches('"').trim_matches('\'');
            if name.is_empty() || GENERIC_FAMILIES.contains(&name) {
                continue;
            }
            assert!(
                embedded.iter().any(|e| e == name),
                "line {}: font-family names {name:?}, which no @font-face ships. \
                 An unbundled face does not error — it falls back silently, which \
                 is exactly how Beamer shipped a \"Cascadia Code\" stack that never \
                 rendered on Linux.",
                i + 1
            );
        }
    }
}

#[test]
fn every_class_in_markup_is_defined_in_the_stylesheet() {
    let mut missing: Vec<String> = Vec::new();

    for (file, src) in MARKUP {
        for class in class_literals(src) {
            // A dynamic segment is resolved at runtime; the literal halves of
            // the surrounding string are still checked.
            if class.contains('{') || class.is_empty() {
                continue;
            }
            let defined = CSS.contains(&format!(".{class}"));
            if !defined {
                missing.push(format!("{file}: .{class}"));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "these classes appear in markup but nothing defines them. An unstyled \
         class does not error or warn — the element simply renders wrong:\n  {}",
        missing.join("\n  ")
    );
}

/// Pull class tokens out of `class:` attributes, covering both the plain
/// `class: "a b"` form and the `class: if cond { "a" } else { "b" }` form.
fn class_literals(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if !line.contains("class:") {
            continue;
        }
        // The if/else form spills onto the same line in practice, but take a
        // small window so a wrapped one is still covered.
        let window = lines[i..(i + 2).min(lines.len())].join(" ");
        let Some((_, after)) = window.split_once("class:") else {
            continue;
        };
        for token in attribute_span(after).split_whitespace() {
            out.push(token.to_string());
        }
    }
    out
}

/// The text of one rsx attribute value: everything up to the comma that ends
/// it, with braces respected.
///
/// Stopping at that comma is what separates the class list from the element's
/// own text content — `span { class: "row-label", "Project" }` must yield
/// `row-label` and never `Project`.
fn attribute_span(after: &str) -> String {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut literals = String::new();

    for ch in after.chars() {
        match ch {
            '"' => {
                // A separator on close, or the two arms of an if/else fuse
                // into one nonexistent class name.
                if in_string {
                    literals.push(' ');
                }
                in_string = !in_string;
            }
            '{' if !in_string => depth += 1,
            '}' if !in_string => depth = depth.saturating_sub(1),
            ',' if !in_string && depth == 0 => break,
            _ if in_string => literals.push(ch),
            _ => {}
        }
    }
    literals
}

#[test]
fn phosphor_appears_only_inside_the_run_strip() {
    let mut escapes: Vec<String> = Vec::new();

    for (selector, block) in rules(CSS) {
        if !block.contains("var(--phos)") {
            continue;
        }
        // A selector list is comma-separated and every branch has to qualify;
        // one stray member is enough to paint neon on cream.
        for part in selector.split(',') {
            let part = part.trim();
            if !part.starts_with(".run-strip") {
                escapes.push(format!("{part} {{ ... var(--phos) ... }}"));
            }
        }
    }

    assert!(
        escapes.is_empty(),
        "#1AFC44 has relative luminance 0.70 — 1.4:1 against white — so it is \
         legible only on the dark instrument surface. These rules use it \
         elsewhere:\n  {}",
        escapes.join("\n  ")
    );
}

#[test]
fn the_transparent_window_ground_is_still_declared() {
    // The window is undecorated and transparent, with .app-container painting
    // the background and the corner radius. Without this the webview paints an
    // opaque white square over the rounded corners.
    let normalised: String = CSS.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        normalised.contains("html,body,#main{background:transparent"),
        "html, body and #main must stay transparent or the rounded corners are \
         covered by a white square"
    );
}
