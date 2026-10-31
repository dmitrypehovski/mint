//! The skill compiler.
//!
//! A first run — whether the model decides or a person demonstrates — produces a program,
//! not a recording. Each step describes the *shape* of the thing to act on: its role, the
//! shape of where it points, and its place among the elements that share that shape. Shape
//! is what survives the contents changing, and surviving that is the difference between a
//! skill and a macro.
//!
//! A step also remembers the page it produced, so a later run can tell a step that worked
//! from one that quietly did something else.

use crate::cdp::Element;
use crate::state::{host_of, href_kind, name_shape, page_kind, word_score};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    Click,
    Type { text: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub action: Action,
    pub role: String,
    /// Shape of where the element points. Empty when it points nowhere.
    pub href_kind: String,
    /// Shape of its label, used when there is no href to go by.
    pub name_shape: String,
    /// Which of the elements sharing that shape this step means. On a list page this is
    /// what lets "the newest commit" keep meaning the newest commit after the list changed.
    pub ordinal: usize,
    /// The words seen at learning time — evidence, shown in the report.
    pub seen_label: String,
    /// The page this step was chosen on.
    pub learned_page: String,
    /// The page it produced.
    pub produced_page: String,
    pub intent: String,
    /// The command this step runs, on the surface where a step is a command rather than a click.
    /// Absent for a browser step, which is why it is optional rather than a second kind of step: the
    /// shape of a step is the same on both surfaces, and only what it acts with differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    pub confidence: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Skill {
    pub goal: String,
    pub learned_from: String,
    pub host: String,
    pub expect_final_page: String,
    pub steps: Vec<Step>,
    pub learn_model_calls: u32,
    pub learn_ms: u64,
    /// Did the run that produced this skill actually reach the goal, or was it cut short
    /// and is only a guess at a prefix? Never report success on an unverified one.
    #[serde(default)]
    pub learn_declared_done: bool,
}

/// Where a step's element came from. Reported for every step, so a run can be audited
/// rather than believed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Found {
    Shape,
    Words,
    Taught,
}

impl Found {
    pub fn as_str(self) -> &'static str {
        match self {
            Found::Shape => "shape",
            Found::Words => "words",
            Found::Taught => "the hint",
        }
    }
}

/// The shape description of an element, as it would be written down.
/// The site a URL belongs to, exposed so callers can rebuild a page description.
pub fn host_of_url(url: &str) -> String {
    host_of(url)
}

pub fn describe(el: &Element) -> (String, String) {
    let hk = if el.href.is_empty() { String::new() } else { href_kind(&el.href) };
    (hk, name_shape(&el.name))
}

fn has_shape(step: &Step, el: &Element) -> bool {
    if el.role != step.role {
        return false;
    }
    if !step.href_kind.is_empty() {
        return !el.href.is_empty() && href_kind(&el.href) == step.href_kind;
    }
    !step.name_shape.is_empty() && name_shape(&el.name) == step.name_shape
}

/// Where an element sits among the elements that share its shape.
pub fn ordinal_of(screen: &[Element], el: &Element) -> usize {
    let (hk, ns) = describe(el);
    screen
        .iter()
        .filter(|e| e.role == el.role && if !hk.is_empty() { !e.href.is_empty() && href_kind(&e.href) == hk } else { name_shape(&e.name) == ns })
        .position(|e| e.handle == el.handle)
        .unwrap_or(0)
}

/// Build a step from an element — used both while learning and while repairing.
pub fn step_from(el: &Element, action: Action, intent: &str, confidence: f32) -> Step {
    let (hk, ns) = describe(el);
    Step {
        cmd: None,
        action,
        role: el.role.clone(),
        href_kind: hk,
        name_shape: ns,
        ordinal: 0,
        seen_label: el.name.clone(),
        learned_page: String::new(),
        produced_page: String::new(),
        intent: intent.to_string(),
        confidence,
    }
}

impl Skill {
    pub fn save(&self, path: &str) -> anyhow::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: &str) -> anyhow::Result<Skill> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Which element does this step mean, here and now?
    ///
    /// Shape first: every element sharing the described shape, in document order, take the
    /// one at the remembered place. If nothing on this page has that shape, fall back to
    /// the words — a weaker claim, and reported as such. Nothing at all means None, and the
    /// caller must ask rather than guess.
    pub fn resolve(&self, step: &Step, state: &[Element]) -> Option<(usize, f32, Found)> {
        self.resolve_explained(step, state).one()
    }

    /// Find the element a step means, and say so when the answer is not one element.
    ///
    /// The place a step stood in is the right handle when a page's data has changed: on a second run
    /// the rows carry different words by design, and what identifies the step is that it is the third
    /// of its kind. So the place is taken in preference to the words — but not on trust.
    ///
    /// The test is sharp rather than a matter of degree: if the element standing in that place does
    /// not carry the words that were recorded, while some *other* element on the page carries them
    /// exactly, then the place and the words are pointing at two different things. A person in that
    /// position stops and asks rather than clicking one of them and hoping.
    pub fn resolve_explained(&self, step: &Step, state: &[Element]) -> Resolution {
        let shaped: Vec<usize> = state
            .iter()
            .enumerate()
            .filter(|(_, e)| has_shape(step, e))
            .map(|(i, _)| i)
            .collect();
        let recorded = step.seen_label.trim();

        if let Some(i) = shaped.get(step.ordinal).copied() {
            let here = state[i].name.trim();
            // Exact, not scored. Digits are masked on the way into a shape, which is what lets a step
            // outlive the data it was learned on — and it also means a score cannot tell "row 0007"
            // from "row 0002". The whole string can.
            if !recorded.is_empty() && here != recorded {
                if let Some(j) = state
                    .iter()
                    .position(|e| e.role == step.role && e.name.trim() == recorded)
                {
                    if j != i {
                        return Resolution::Ambiguous {
                            why: format!(
                                "the place this step stood in holds {} «{}», while {} «{}» carries the words \
                                 that were recorded",
                                state[i].role,
                                here.chars().take(30).collect::<String>(),
                                state[j].role,
                                state[j].name.chars().take(30).collect::<String>()
                            ),
                        };
                    }
                }
            }
            return Resolution::One(i, 1.0, Found::Shape);
        }

        // Nothing stands in that place any more. Go by the words, but only when one candidate is
        // clearly ahead of the rest: two that fit equally well are not a match, they are a choice.
        let mut ranked: Vec<(usize, f32)> = state
            .iter()
            .enumerate()
            .filter(|(_, e)| e.role == step.role)
            .map(|(i, e)| (i, word_score(e, recorded)))
            .filter(|(_, sc)| *sc >= 0.5)
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if let Some((i, best)) = ranked.first().copied() {
            let equally_good = ranked
                .iter()
                .filter(|(j, sc)| *j != i && (best - *sc) < 0.1)
                .count();
            if equally_good > 0 {
                return Resolution::Ambiguous {
                    why: format!(
                        "{} elements on this page fit «{}» about equally well (best {:.2}), and the place \
                         this step stood in is gone",
                        equally_good + 1,
                        recorded.chars().take(30).collect::<String>(),
                        best
                    ),
                };
            }
            return Resolution::One(i, best, Found::Words);
        }
        Resolution::Unrecognised
    }

    /// Does this program claim to work on the page in front of us?
    pub fn applies_to(&self, url: &str, state: &[Element]) -> bool {
        host_of(url) == self.host && self.steps.iter().all(|s| self.resolve(s, state).is_some())
    }

    /// Rewrite one step from what was just chosen, so the next run does not have to ask.
    /// A mistake then costs one question instead of a broken run.
    pub fn teach(&mut self, index: usize, el: &Element, screen: &[Element], here: &str, produced: &str) {
        let (hk, ns) = describe(el);
        let ordinal = ordinal_of(screen, el);
        if let Some(step) = self.steps.get_mut(index) {
            step.role = el.role.clone();
            step.href_kind = hk;
            step.name_shape = ns;
            step.ordinal = ordinal;
            step.seen_label = el.name.clone();
            step.learned_page = page_kind(here);
            step.produced_page = produced.to_string();
        }
    }
}

/// What looking for a step's element produced.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// One element is the target, and this is how it was found.
    One(usize, f32, Found),
    /// Two candidates fit, or the place and the words point at different elements. Acting here would
    /// be a guess, so it is refused and named instead.
    Ambiguous { why: String },
    /// Nothing on this page fits.
    Unrecognised,
}

impl Resolution {
    /// The old shape of the answer, for callers that treat every failure alike.
    pub fn one(self) -> Option<(usize, f32, Found)> {
        match self {
            Resolution::One(i, c, how) => Some((i, c, how)),
            _ => None,
        }
    }
}

/// Turn a trail of decisions into a program. `screens` holds the screen each step was
/// chosen from, in order, so each step's place among its peers can be recorded.
pub fn compile(
    goal: &str,
    learned_from: &str,
    trail: &[(Step, Element)],
    screens: &[Vec<Element>],
    learn_model_calls: u32,
    learn_ms: u64,
    final_page_kind: &str,
) -> Skill {
    let steps = trail
        .iter()
        .enumerate()
        .map(|(i, (step, el))| {
            let mut s = step.clone();
            let (hk, ns) = describe(el);
            s.role = el.role.clone();
            s.href_kind = hk;
            s.name_shape = ns;
            s.seen_label = el.name.clone();
            s.ordinal = screens.get(i).or_else(|| screens.last()).map(|sc| ordinal_of(sc, el)).unwrap_or(0);
            s
        })
        .collect();
    Skill {
        goal: goal.to_string(),
        learned_from: learned_from.to_string(),
        host: host_of(learned_from),
        expect_final_page: final_page_kind.to_string(),
        steps,
        learn_model_calls,
        learn_ms,
        learn_declared_done: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(role: &str, name: &str) -> Element {
        Element {
            handle: "h".into(),
            tag: "div".into(),
            role: role.into(),
            name: name.into(),
            href: String::new(),
            on: true,
            checked: None,
            value: None,
        }
    }

    fn empty_skill() -> Skill {
        compile("test goal", "https://example.com/fixture", &[], &[], 0, 0, "")
    }

    fn a_step(role: &str, label: &str, ordinal: usize) -> Step {
        let e = el(role, label);
        let mut st = step_from(&e, Action::Click, "test", 1.0);
        st.seen_label = label.to_string();
        st.ordinal = ordinal;
        st
    }

    #[test]
    fn the_place_a_step_stood_in_is_honoured_when_the_data_changed() {
        // A second run of the same task on a folder full of different files: no row carries the words
        // that were recorded, which is the whole reason the place is recorded as well.
        let sk = empty_skill();
        let step = a_step("row", "invoice-2026-01-0100.pdf", 1);
        let page = vec![
            el("row", "invoice-2026-05-0211.pdf"),
            el("row", "invoice-2026-09-0388.pdf"),
            el("row", "invoice-2026-12-0912.pdf"),
        ];
        assert_eq!(
            sk.resolve_explained(&step, &page),
            Resolution::One(1, 1.0, Found::Shape)
        );
    }

    #[test]
    fn a_page_that_moved_the_target_aside_is_refused_rather_than_guessed_at() {
        let sk = empty_skill();
        let step = a_step("row", "row 0007", 1);
        // The place holds a different row, and the row that was recorded is somewhere else.
        let page = vec![
            el("row", "row 0001"),
            el("row", "row 0002"),
            el("row", "row 0007"),
        ];
        match sk.resolve_explained(&step, &page) {
            Resolution::Ambiguous { why } => {
                assert!(why.contains("the place this step stood in holds"), "{why}");
                assert!(why.contains("row 0007"), "{why}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn two_candidates_that_fit_equally_well_are_not_a_match() {
        let sk = empty_skill();
        // A place that no longer exists, and two buttons that both answer to the recorded words.
        let step = a_step("button", "continue to the next page", 9);
        let page = vec![
            el("button", "continue to the next page"),
            el("button", "continue to the next page"),
        ];
        match sk.resolve_explained(&step, &page) {
            Resolution::Ambiguous { why } => assert!(why.contains("equally well"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn nothing_that_fits_is_not_the_same_as_ambiguous() {
        let sk = empty_skill();
        let step = a_step("button", "a button that is not here", 0);
        let page = vec![el("row", "unrelated"), el("link", "also unrelated")];
        assert_eq!(sk.resolve_explained(&step, &page), Resolution::Unrecognised);
    }
}
