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
        let shaped: Vec<usize> = state
            .iter()
            .enumerate()
            .filter(|(_, e)| has_shape(step, e))
            .map(|(i, _)| i)
            .collect();
        if let Some(&i) = shaped.get(step.ordinal) {
            return Some((i, 1.0, Found::Shape));
        }
        let mut best: Option<(usize, f32)> = None;
        for (i, el) in state.iter().enumerate() {
            if el.role != step.role {
                continue;
            }
            let s = word_score(el, &step.seen_label);
            if s > 0.0 && best.map(|(_, b)| s > b).unwrap_or(true) {
                best = Some((i, s));
            }
        }
        best.filter(|(_, s)| *s >= 0.5).map(|(i, s)| (i, s, Found::Words))
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
