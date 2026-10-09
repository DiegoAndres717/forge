// Project Guard: reglas deterministas antes de commit, push y pull request.
// Independiente de la UI: la app, los hooks de Git y la CLI usan el mismo motor.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::candidate::{self, Candidate, Snapshot};
use crate::evidence::{Evidence, Report, ReportCheck, ReportItem};
use crate::reviewers::Activation;
use crate::router::{self, AiOutcome, AiVerdict, ReviewContext, RouterConfig, Usage};
use sha2::{Digest, Sha256};

pub(crate) mod diff;
mod evaluate;
mod rules;
mod run;
mod secrets;

pub use diff::*;
pub use evaluate::*;
pub use rules::*;
pub use run::*;
pub use secrets::*;

#[cfg(test)]
mod tests;
