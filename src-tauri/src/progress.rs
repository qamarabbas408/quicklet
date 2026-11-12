//! Progress reporting and cooperative cancellation for file operations.
//!
//! Operation modules call [`Progress::step`] between phases. A step reports how
//! far the current stage has got and returns [`Cancelled`] once the user asked
//! to stop. Modules must not call `step` after they start writing output, so a
//! cancelled operation never leaves a partial file behind.

/// Returned by [`Progress::step`] once the operation should stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

pub trait Progress {
    /// Reports that `stage` reached `completed` of `total`. A `total` of 0
    /// means the stage has no measurable size.
    fn step(
        &mut self,
        stage: &'static str,
        completed: usize,
        total: usize,
    ) -> Result<(), Cancelled>;
}

/// Ignores progress and never cancels.
impl Progress for () {
    fn step(&mut self, _: &'static str, _: usize, _: usize) -> Result<(), Cancelled> {
        Ok(())
    }
}

/// Implemented by operation errors so callers can tell a user cancellation
/// apart from a failure.
pub trait OperationError: std::fmt::Display {
    fn is_cancelled(&self) -> bool;
}

#[cfg(test)]
pub(crate) mod testing {
    use super::{Cancelled, Progress};

    /// Records every step and cancels once `cancel_at` steps have been seen.
    #[derive(Default)]
    pub struct Recorder {
        pub steps: Vec<(&'static str, usize, usize)>,
        pub cancel_at: Option<usize>,
    }

    impl Recorder {
        pub fn cancelling_at(step: usize) -> Self {
            Self {
                steps: Vec::new(),
                cancel_at: Some(step),
            }
        }
    }

    impl Progress for Recorder {
        fn step(
            &mut self,
            stage: &'static str,
            completed: usize,
            total: usize,
        ) -> Result<(), Cancelled> {
            if self.cancel_at == Some(self.steps.len()) {
                return Err(Cancelled);
            }
            self.steps.push((stage, completed, total));
            Ok(())
        }
    }
}
