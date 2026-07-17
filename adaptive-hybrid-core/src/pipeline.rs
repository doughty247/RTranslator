//! Live-direction and push-to-talk-direction state machines.
//!
//! Not implemented yet — this is the core design contribution
//! `docs/adaptive-hybrid-mode-design.md` (Phase 3) exists to work out before
//! any code here is written: how a live direction's continuous VAD-gated loop
//! and a push-to-talk direction's button-triggered loop coexist without
//! fighting over shared state, and how audio chunks from the single Java-side
//! `AudioRecord` session get routed to the right internal loop. Implementing
//! ahead of that review would mean guessing at the developer's own design
//! contribution, which `CLAUDE.md` is explicit should not happen.
