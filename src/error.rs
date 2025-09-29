use crate::fs::Loader;
use crate::parse::Rule;
use core::fmt::{Debug, Display, Formatter};
use core::ops::Range;
use pest::error::{Error, ErrorVariant};
use std::io::Write;
use std::path::Path;

/// An error annotated with a span of source code.
pub struct SourceError {
    /// The wrapped error.
    pub pest_error: Box<Error<Rule>>,
    /// The source's address range; effectively, a &'static str which can only be compared by
    /// identity.  This is used to reconstruct the source path during error reporting.
    pub buffer: Range<usize>,
}

impl SourceError {
    pub fn new(message: String, span: pest::Span) -> Self {
        let message = ErrorVariant::CustomError { message };
        let buffer = span.get_input().as_bytes().as_ptr_range();
        let buffer = buffer.start as usize..buffer.end as usize;
        let pest_error = Box::new(Error::new_from_span(message, span));
        Self { pest_error, buffer }
    }

    // TODO: better to do this with an enum field.  the formatting looks strange this way.
    pub fn new_unattributed(message: String) -> Self {
        let message = ErrorVariant::CustomError { message };
        let src = "";
        let buffer = src.as_bytes().as_ptr_range();
        let buffer = buffer.start as usize..buffer.end as usize;
        let pos = pest::Position::from_start(src);
        let pest_error = Box::new(Error::new_from_pos(message, pos));
        Self { pest_error, buffer }
    }

    pub fn path(&self) -> Option<&str> {
        self.pest_error.path()
    }

    pub fn with_path(mut self, path: &Path) -> Self {
        *self.pest_error = self.pest_error.with_path(&path.to_string_lossy());
        self
    }

    pub fn buffer(&self) -> Range<*const u8> {
        self.buffer.start as *const u8..self.buffer.end as *const u8
    }
}

impl From<Error<Rule>> for SourceError {
    fn from(pest_error: Error<Rule>) -> Self {
        let pest_error = Box::new(pest_error);
        let buffer = "".as_bytes().as_ptr_range();
        let buffer = buffer.start as usize..buffer.end as usize;
        SourceError { pest_error, buffer }
    }
}

impl Debug for SourceError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        Display::fmt(&self.pest_error, f)
    }
}

impl Display for SourceError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        Display::fmt(&self.pest_error, f)
    }
}

impl core::error::Error for SourceError {}

/// A receiver for warnings and errors generated during a lengthy computation.  The idea is to
/// allow evaluation to proceed as far as possible while still capturing any errors that occur.
/// A Scribe cannot be dropped; `into_inner()` or `report()` must be called.
#[derive(Default)]
pub struct Scribe {
    warnings_are_errors: bool,
    warnings: Vec<SourceError>,
    errors: Vec<SourceError>,
}

impl Scribe {
    pub fn new(warnings_are_errors: bool) -> Self {
        Self {
            warnings_are_errors,
            warnings: vec![],
            errors: vec![],
        }
    }

    /// Report a warning.
    pub fn warn(&mut self, err: SourceError) {
        if self.warnings_are_errors {
            self.errors.push(err);
        } else {
            self.warnings.push(err);
        }
    }

    /// Report an error.
    pub fn err(&mut self, err: SourceError) {
        self.errors.push(err);
    }

    /// Extract the logged warnings and errors.
    pub fn into_inner(mut self) -> (Vec<SourceError>, Vec<SourceError>) {
        let mut warnings = vec![];
        let mut errors = vec![];
        core::mem::swap(&mut warnings, &mut self.warnings);
        core::mem::swap(&mut errors, &mut self.errors);
        core::mem::forget(self);
        (warnings, errors)
    }

    /// Extract the first logged error, if any.
    pub fn collect(self) -> Result<(), SourceError> {
        let (_warnings, mut errors) = self.into_inner();
        errors.truncate(1);
        errors.pop().map_or(Ok(()), Err)
    }

    /// Print the logged warning and errors to the supplied sink.
    /// (Warnings are printed only if no errors occurred.)
    /// Returns false if errors occurred, or true otherwise.
    #[must_use]
    pub fn report(self, loader: &impl Loader, console: &mut impl Write) -> bool {
        let (warnings, errors) = self.into_inner();
        if errors.is_empty() {
            for err in warnings {
                _ = writeln!(console, "Warning: {}", loader.annotate_error(err));
            }
            true
        } else {
            for err in errors {
                _ = writeln!(console, "Error: {}", loader.annotate_error(err));
            }
            false
        }
    }
}

impl Drop for Scribe {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            panic!("Scribe must be consumed explicitly");
        }
    }
}
