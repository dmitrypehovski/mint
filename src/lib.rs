//! mint as a library.
//!
//! The runtime is one program built out of four pieces that are separable on purpose. Anybody who
//! wants the write gate, the receipts, the rules document or the hash chain inside their own agent
//! should be able to take that piece and leave the rest:
//!
//! - [`gate`] — decides whether a step may run, before it runs. The gate is an argument of the one
//!   function that can touch a page, so it cannot be called around; removing it is a build error.
//! - [`receipt`] — a signed, chained record of what was decided and what then happened. Two
//!   receipts for a performed action, one for a refusal, and the absence of the second is itself
//!   evidence that nothing took place.
//! - [`policy`] — the rules as a document rather than as code, so that a verifier can re-derive
//!   every verdict from the observations the receipts actually recorded.
//! - [`endorse`] — a second party's signature over those rules, bound to the policy hash, the
//!   runtime and an optional expiry. A runtime must never produce its own.
//!
//! The agent itself is [`cdp`] (the browser), [`skill`] (the compiled task), [`state`] (comparing
//! live state against what a step expects) and [`decide`] (the model adapter).
//!
//! # Embedding it
//!
//! The three pieces worth embedding stand alone:
//!
//! ```no_run
//! use mint::policy::{ruling, Observed, Policy};
//!
//! let rules = Policy::shipped();
//! let observed = Observed {
//!     action: "click".into(),
//!     tag: "button".into(),
//!     role: "button".into(),
//!     has_href: false,
//!     approval: false,
//! };
//! let said = ruling(&observed, &rules);
//! assert_eq!(said.verdict, "hold");
//! ```
//!
//! Nothing above needs a browser, a model or a network. `cargo run -- rules --policy policy.json
//! --observations observations.json` does the same thing from a shell, and is what the
//! demonstration site calls when a visitor flips a switch.

pub mod cdp;
pub mod decide;
pub mod endorse;
pub mod gate;
pub mod policy;
pub mod receipt;
pub mod shell;
pub mod skill;
pub mod state;
