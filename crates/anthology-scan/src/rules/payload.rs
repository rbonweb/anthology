//! Payloads: content that does not match its name, executables, blobs, decode-then-exec (Task 4).

use crate::{FileCtx, Finding};

pub(crate) fn check(_f: &FileCtx, _out: &mut Vec<Finding>) {}
