//! State comparison and element matching.
//!
//! Two jobs: make a screen cheap to compare, so that "nothing has changed" is recognised
//! instantly, and describe an element by its *shape* rather than by its contents — the
//! difference between a program that works only on the page it was learned on and one
//! that works on the next page of the same kind.

use crate::cdp::Element;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

/// A cheap, order-independent fingerprint of what is on screen.
pub fn signature(url: &str, els: &[Element]) -> u64 {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for e in els.iter().take(80) {
        set.insert(format!("{}|{}", e.role, e.name));
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path_of(url).hash(&mut h);
    for item in set {
        item.hash(&mut h);
    }
    h.finish()
}

/// The part of a URL that identifies the screen, with volatile query values removed.
pub fn path_of(url: &str) -> String {
    let no_scheme = url.splitn(2, "//").nth(1).unwrap_or(url);
    let path = no_scheme.split(['?', '#']).next().unwrap_or(no_scheme);
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        no_scheme.split(['?', '#']).next().unwrap_or(no_scheme).to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn host_of(url: &str) -> String {
    let no_scheme = url.splitn(2, "//").nth(1).unwrap_or(url);
    no_scheme.split(['/', '?']).next().unwrap_or(no_scheme).to_lowercase()
}

/// Is this path segment an identifier — a hash, a number, a slug nobody types twice?
fn is_id_like(seg: &str) -> bool {
    // A name that says what the page is — "web-form.html", "Internet_Computer", "pull-requests" —
    // has to survive being masked. Two pages are often told apart by exactly that name, and masking
    // it makes them look like one page, so a step that worked is thrown away as "nothing changed".
    if seg.chars().all(|c| c.is_ascii_digit()) {
        return true; // an issue number, a page number
    }
    // A commit hash or a uuid: hex letters, digits and dashes, and long enough to be opaque.
    if seg.len() >= 7 && seg.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return true;
    }
    // A long run of letters and digits with nothing separating it, with both kinds in it: opaque.
    if !seg.contains('.') && !seg.contains('-') && !seg.contains('_') && seg.len() >= 12 {
        return seg.chars().any(|c| c.is_ascii_digit()) && seg.chars().any(char::is_alphabetic);
    }
    false
}

/// The shape of where an element points.
///
/// The last two path segments are kept, with identifier-like ones replaced by `*`. That is
/// enough to say "the commit link" or "the commits tab" without saying *which* repository —
/// which is exactly the difference between a remembered step and a memorised one.
pub fn href_kind(href: &str) -> String {
    let path = href.split(['?', '#']).next().unwrap_or(href);
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let tail: Vec<String> = segs
        .iter()
        .rev()
        .take(2)
        .rev()
        .map(|s| if is_id_like(s) { "*".to_string() } else { s.to_lowercase() })
        .collect();
    tail.join("/")
}

/// The shape of a label: lowercased, punctuation stripped, numbers made anonymous. Used
/// for elements that have no `href` to describe them.
pub fn name_shape(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.split_whitespace()
        .map(|w| if w.chars().all(|c| c.is_ascii_digit()) { "*" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

fn tokens(s: &str) -> BTreeSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(|t| t.to_string())
        .collect()
}

/// How well an element answers remembered words. The last resort: shape first, words only
/// when nothing on the page has the shape.
pub fn word_score(el: &Element, want: &str) -> f32 {
    let want_t = tokens(want);
    let have = tokens(&el.name);
    if want_t.is_empty() || have.is_empty() {
        return 0.0;
    }
    let hit = want_t.intersection(&have).count() as f32;
    let score = hit / want_t.len() as f32;
    if el.name.to_lowercase().contains(&want.to_lowercase()) { (score + 0.5).min(1.5) } else { score }
}

/// What changed between two observations, described for a reader.
pub fn diff(before: &[Element], after: &[Element]) -> Vec<String> {
    let key = |e: &Element| format!("{}|{}", e.role, e.name);
    let b: BTreeSet<String> = before.iter().map(key).collect();
    let a: BTreeSet<String> = after.iter().map(key).collect();
    let mut out = Vec::new();
    for added in a.difference(&b).take(8) {
        out.push(format!("+ {added}"));
    }
    for gone in b.difference(&a).take(8) {
        out.push(format!("- {gone}"));
    }
    out
}


/// The kind of page a URL is, expressed the way links are: the segments after the
/// site-and-project prefix, with identifiers replaced by `*`. Two repositories' roots are
/// the same kind of page; a commit page is a different kind. This is what lets a step's
/// expectation survive the data changing.
pub fn page_kind(url: &str) -> String {
    let p = path_of(url);
    let after_host = p.splitn(2, '/').nth(1).unwrap_or("");
    let segs: Vec<String> = after_host
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| if is_id_like(s) { "*".to_string() } else { s.to_lowercase() })
        .collect();
    match segs.len() {
        // No path at all, or one segment: that segment *is* the page.
        0 => String::new(),
        1 => segs[0].clone(),
        // Two segments is a site's project root — the same kind of page whatever the
        // project is called. This is what lets a skill move between repositories.
        2 => String::new(),
        // Deeper: everything after the project prefix describes the page itself.
        _ => segs[2..].join("/"),
    }
}

/// The last segment of the address, masked if it looks like an identifier.
///
/// `page_kind` erases the second segment on purpose: two segments are a site's project root, and that
/// is what lets a skill learned on one repository run on another. The cost is that a page like
/// `/fieldwork/submit` has no kind of its own, and every address at that depth looks like the same
/// place. When a run ends there, this is what the page can be named by.
pub fn page_kind_deep(url: &str) -> String {
    let p = path_of(url);
    let seg = p.split('/').filter(|s| !s.is_empty()).next_back().unwrap_or("");
    if seg.is_empty() || is_id_like(seg) {
        String::new()
    } else {
        seg.to_lowercase()
    }
}

/// Whether the address we are on is the page a run ended on.
///
/// Two forms are accepted: the kind the runtime normally uses, and the last segment, for addresses
/// that have no kind of their own. A skill pinned by the first form keeps working across projects; a
/// skill that can only be pinned by the second is still checked rather than waved through.
pub fn landed_on(url: &str, expect: &str) -> bool {
    if expect.is_empty() {
        return false;
    }
    page_kind(url) == expect || page_kind_deep(url) == expect
}

/// One line of the table: its place in the full list, its role, its name, where it points, whether it
/// can be reached from where it sits, and what it already holds.
fn element_line(i: usize, e: &Element) -> String {
    let where_to = if e.href.is_empty() { String::new() } else { format!(" → {}", href_kind(&e.href)) };
    // Marked, not hidden: an element that exists but cannot be reached is a fact about the page, and
    // the model does better knowing it than wondering why a click did nothing.
    let reach = if e.on { "" } else { " [not reachable here]" };
    // What the field already holds, and whether a box is ticked. A form is filled once, and a model
    // that cannot see its own work fills it in circles: this is the whole reason a long form is
    // possible at all.
    let holds = match (&e.value, e.checked) {
        (Some(v), _) if !v.is_empty() => {
            format!(" [= {}]", v.chars().take(40).collect::<String>())
        }
        (_, Some(true)) => " [ticked]".to_string(),
        (_, Some(false)) => " [not ticked]".to_string(),
        _ => String::new(),
    };
    format!(
        "{}: {} \"{}\"{}{}{}\n",
        i, e.role, e.name, where_to, reach, holds
    )
}

/// The whole screen. This is what the judge is given: it has to see the finished work, not the
/// remaining work.
pub fn as_table(els: &[Element]) -> String {
    let mut s = String::new();
    for (i, e) in els.iter().enumerate() {
        s.push_str(&element_line(i, e));
    }
    s
}

/// Whether this element is something a person types into.
pub fn is_field(e: &Element) -> bool {
    e.role == "textbox" || e.role == "searchbox"
}

/// Whether this field is still waiting for something.
pub fn is_empty_field(e: &Element) -> bool {
    is_field(e) && e.value.as_deref().map(|v| v.trim().is_empty()).unwrap_or(true)
}

/// The screen as the deciding model sees it: the work that is left.
///
/// A form of fifty fields with thirty-five of them filled is a wall. The model has to find the
/// fifteen that are not, count them, and carry the count to the next step — with a trail six steps
/// long to carry it by. It gives up in the most plausible way available: it presses Send and calls
/// the task finished, which is exactly what happened here twice, at thirty-five of fifty.
///
/// So the filled fields are left out and only the next `budget` empty ones are shown. The position
/// on the page is then re-read from the page on every step instead of being remembered, which is how
/// the fifty-invoice folder task works: each step removes the thing it acted on. The indices are the
/// real ones for the full list, so a step chosen here is the same step as before.
pub fn as_table_remaining(els: &[Element], budget: usize) -> String {
    let mut s = String::new();
    let mut shown = 0usize;
    let mut hidden = 0usize;
    let mut filled = 0usize;
    for (i, e) in els.iter().enumerate() {
        if !is_field(e) {
            // Everything that is not a field is always shown, whatever it is doing: buttons, links,
            // tick-boxes and radios are how the run moves, and hiding one would hide the way on.
            s.push_str(&element_line(i, e));
            continue;
        }
        if !is_empty_field(e) {
            filled += 1;
            continue;
        }
        if shown < budget {
            s.push_str(&element_line(i, e));
            shown += 1;
        } else {
            hidden += 1;
        }
    }
    if filled > 0 {
        s.push_str(&format!(
            "({filled} filled field(s) are not listed — they are done and need nothing.)\n"
        ));
    }
    if hidden > 0 {
        s.push_str(&format!(
            "({hidden} more empty field(s) follow the ones above; they appear as these are filled.)\n"
        ));
    }
    s
}
