//! Plover's Python dictionaries, run as they are.
//!
//! A Plover Python dictionary is an ordinary `.py` file defining:
//!
//! - `LONGEST_KEY`, how many strokes it can answer for
//! - `lookup(key)`, where `key` is a tuple of outline strings, returning the
//!   translation or **raising `KeyError`** to mean "no entry"
//! - optionally `reverse_lookup(text)`, returning a list of outlines
//!
//! Running them rather than porting them was a deliberate choice, and it was
//! made on measurement. `jeff-phrasing.py` answers a lookup in 2.2us and misses
//! in 1.2us, against 0.4us to 6.5us for this project's own Rust JSON lookups
//! and roughly 200,000us between strokes at 300wpm. Python is not the slow part
//! of anything here, so porting buys speed nobody can perceive and costs a
//! rewrite per dictionary that can silently drift from the original.
//!
//! **This is the same trust model as Plover.** A Python dictionary is arbitrary
//! code with no sandbox. Running one is running whatever its author wrote.
//!
//! **Embedding ties the executable to a Python installation, but not to one
//! version.** Built against the CPython stable ABI (`abi3-py312`), so the exe
//! links the version-independent `python3.dll` with 3.12 as the floor and runs
//! against any CPython 3.12 or newer that is installed. The one runtime
//! requirement is that `python3.dll` is findable on the DLL search path, which a
//! standard CPython install on `PATH` provides. If no CPython 3.12+ is present,
//! the exe stops starting, which is the cost of the generality this buys.

use std::path::{Path, PathBuf};

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use pluvialis_core::ProgrammaticDictionary;

#[derive(Debug, thiserror::Error)]
pub enum PythonError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path}: {source}")]
    Python {
        path: PathBuf,
        #[source]
        source: PyErr,
    },

    #[error("{path}: no lookup function, so this cannot act as a dictionary")]
    NoLookup { path: PathBuf },
}

/// Does this source define `lookup` at all?
///
/// Checked **before** the module is executed, and that ordering is the whole
/// point. Plover only ever runs a `.py` that its config names as a dictionary;
/// Pluvialis finds them by scanning the dictionary folder, which is friendlier
/// but means it meets files that were never meant to be dictionaries. A real
/// folder can hold such things: a backup script that copies a dictionary to
/// another drive from module level, or one that rewrites a dictionary in place.
/// Executing first and asking questions afterwards would run the copy on every
/// start.
///
/// This is not a sandbox and is not trying to be. Anything that passes is still
/// arbitrary code, exactly as in Plover. It only keeps the app from running
/// files nobody asked it to run.
/// Could this source act as a Plover dictionary, judged without running it?
///
/// For callers that need to sort dictionaries from ordinary scripts before
/// deciding what to do with a file. See [`defines_lookup`] for why the question
/// has to be answerable without executing anything.
///
/// A `true` here is not a promise that the file loads. It only means executing
/// it is a reasonable thing to attempt.
pub fn looks_like_a_dictionary(source: &str) -> bool {
    defines_lookup(source)
}

fn defines_lookup(source: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("def lookup(")
            || line.starts_with("def lookup (")
            || line.starts_with("lookup =")
            || line.starts_with("lookup=")
            // `from module import lookup`, and the aliased form.
            || (line.starts_with("from ") && line.contains(" import ") && {
                let imports = line.split(" import ").nth(1).unwrap_or("");
                imports
                    .split(',')
                    .any(|name| name.trim().trim_end_matches(')') == "lookup"
                        || name.trim().ends_with(" as lookup"))
            })
    })
}

/// A loaded Plover Python dictionary.
pub struct PythonDictionary {
    /// The module's namespace, holding `lookup` and friends. Kept alive for the
    /// life of the dictionary so the module is executed once rather than per
    /// lookup.
    namespace: Py<PyDict>,
    path: PathBuf,
    longest_key: usize,
    has_reverse: bool,
    enabled: bool,
}

impl PythonDictionary {
    /// Load and execute a `.py` dictionary.
    ///
    /// The module body runs here, so a syntax error or a failed import is
    /// reported at load time rather than on the first stroke that reaches it.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PythonError> {
        let path = path.as_ref().to_path_buf();

        // UTF-8, like every other file this project reads.
        let source = std::fs::read_to_string(&path).map_err(|source| PythonError::Io {
            path: path.clone(),
            source,
        })?;

        // Before running anything. See `defines_lookup`.
        if !defines_lookup(&source) {
            return Err(PythonError::NoLookup { path });
        }

        Python::attach(|py| {
            let namespace = PyDict::new(py);

            // __file__ and __name__ so a dictionary that inspects either (some
            // load data files relative to themselves) behaves as it would
            // under Plover.
            let py_error = |source| PythonError::Python {
                path: path.clone(),
                source,
            };
            namespace
                .set_item("__file__", path.to_string_lossy().as_ref())
                .map_err(py_error)?;
            namespace
                .set_item("__name__", "pluvialis_dictionary")
                .map_err(py_error)?;

            let code = std::ffi::CString::new(source).map_err(|_| PythonError::Io {
                path: path.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the file contains a NUL byte",
                ),
            })?;

            py.run(code.as_c_str(), Some(&namespace), None)
                .map_err(py_error)?;

            if !namespace
                .get_item("lookup")
                .map_err(py_error)?
                .map(|value| value.is_callable())
                .unwrap_or(false)
            {
                return Err(PythonError::NoLookup { path: path.clone() });
            }

            let has_reverse = namespace
                .get_item("reverse_lookup")
                .map_err(py_error)?
                .map(|value| value.is_callable())
                .unwrap_or(false);

            // Plover's convention. Default to one stroke when a dictionary does
            // not say, which is what a phrasing style dictionary wants.
            let longest_key = namespace
                .get_item("LONGEST_KEY")
                .map_err(py_error)?
                .and_then(|value| value.extract::<usize>().ok())
                .unwrap_or(1)
                .max(1);

            Ok(PythonDictionary {
                namespace: namespace.unbind(),
                path: path.clone(),
                longest_key,
                has_reverse,
                enabled: true,
            })
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn call(&self, name: &str, outlines: &[String]) -> Option<String> {
        Python::attach(|py| {
            let namespace = self.namespace.bind(py);
            let function = namespace.get_item(name).ok()??;

            // Plover passes the outline as a tuple of strings.
            let key = PyTuple::new(py, outlines).ok()?;
            match function.call1((key,)) {
                Ok(value) => value.extract::<Option<String>>().ok().flatten(),
                Err(e) => {
                    // KeyError is how a Python dictionary says "no entry". It is
                    // the ordinary miss path, not a failure, and it happens on
                    // most strokes.
                    if e.is_instance_of::<pyo3::exceptions::PyKeyError>(py) {
                        return None;
                    }
                    // Anything else is a real fault. Report it and treat the
                    // stroke as a miss: one broken dictionary should cost the
                    // stroke it was consulted for, not the session.
                    log::warn!("{}: {name} failed: {e}", self.path.display());
                    None
                }
            }
        })
    }
}

impl ProgrammaticDictionary for PythonDictionary {
    fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    fn longest_key(&self) -> usize {
        self.longest_key
    }

    fn lookup(&self, outlines: &[String]) -> Option<String> {
        self.call("lookup", outlines)
    }

    fn reverse_lookup(&self, text: &str) -> Vec<String> {
        if !self.has_reverse {
            return Vec::new();
        }

        Python::attach(|py| {
            let namespace = self.namespace.bind(py);
            let Ok(Some(function)) = namespace.get_item("reverse_lookup") else {
                return Vec::new();
            };
            match function.call1((text,)) {
                Ok(value) => value.extract::<Vec<String>>().unwrap_or_default(),
                Err(e) => {
                    if !e.is_instance_of::<pyo3::exceptions::PyKeyError>(py) {
                        log::warn!("{}: reverse_lookup failed: {e}", self.path.display());
                    }
                    Vec::new()
                }
            }
        })
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

#[cfg(test)]
mod screening {
    use super::*;

    #[test]
    fn it_accepts_the_shapes_a_dictionary_uses() {
        assert!(defines_lookup(
            "def lookup(key):\n    raise KeyError(key)\n"
        ));
        assert!(defines_lookup("lookup = _make_lookup()\n"));
        assert!(defines_lookup("from tables import lookup\n"));
        assert!(defines_lookup("from tables import phrase as lookup\n"));
        assert!(defines_lookup(
            "if fast:\n    def lookup(key):\n        pass\n"
        ));
    }

    /// A script that merely mentions dictionaries is not one.
    #[test]
    fn it_refuses_a_script_with_no_lookup() {
        assert!(!defines_lookup(
            "import shutil\nshutil.copy2(source, destination)\n"
        ));
        assert!(!defines_lookup("# a lookup would go here\n"));
        assert!(!defines_lookup("result = do_lookup(key)\n"));
    }

    /// Point this at a real dictionary folder that also holds `.py` scripts
    /// which are not dictionaries. A backup script that copies a dictionary to
    /// another drive from module level runs that copy just from being imported,
    /// so screening it out without executing it is the whole point.
    #[test]
    #[ignore = "depends on files outside the repository"]
    fn it_refuses_the_scripts_in_the_users_dictionary_folder() {
        let folder = Path::new(r"C:\Users\you\AppData\Local\plover\plover");
        for name in ["backup-dict.py", "merge_dictionaries.py"] {
            let source = std::fs::read_to_string(folder.join(name)).expect("reading the script");
            assert!(!defines_lookup(&source), "{name} must not be executed");
        }
        let phrasing =
            std::fs::read_to_string(folder.join("jeff-phrasing.py")).expect("reading the phrasing");
        assert!(defines_lookup(&phrasing));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pluvialis-py-{name}.py"));
        std::fs::write(&path, body).expect("writing the test dictionary");
        path
    }

    fn load(name: &str, body: &str) -> Result<PythonDictionary, PythonError> {
        PythonDictionary::load(write(name, body))
    }

    fn outlines(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_dictionary_answers_a_lookup() {
        let dictionary = load(
            "basic",
            "LONGEST_KEY = 1\ndef lookup(key):\n    if key == ('KAT',): return 'cat'\n    raise KeyError\n",
        )
        .expect("load");

        assert_eq!(
            dictionary.lookup(&outlines(&["KAT"])),
            Some("cat".to_owned())
        );
    }

    /// The ordinary miss path, and it happens on most strokes.
    #[test]
    fn a_key_error_means_no_entry_rather_than_a_failure() {
        let dictionary = load(
            "miss",
            "LONGEST_KEY = 1\ndef lookup(key):\n    raise KeyError\n",
        )
        .expect("load");

        assert_eq!(dictionary.lookup(&outlines(&["KAT"])), None);
    }

    #[test]
    fn longest_key_is_read_and_defaults_to_one() {
        let dictionary = load("nokey", "def lookup(key):\n    raise KeyError\n").expect("load");
        assert_eq!(dictionary.longest_key(), 1);

        let dictionary = load(
            "withkey",
            "LONGEST_KEY = 5\ndef lookup(key):\n    raise KeyError\n",
        )
        .expect("load");
        assert_eq!(dictionary.longest_key(), 5);
    }

    #[test]
    fn a_multi_stroke_outline_arrives_as_a_tuple_in_order() {
        let dictionary = load(
            "multi",
            "LONGEST_KEY = 3\ndef lookup(key):\n    return '/'.join(key)\n",
        )
        .expect("load");

        assert_eq!(
            dictionary.lookup(&outlines(&["A", "B", "C"])),
            Some("A/B/C".to_owned())
        );
    }

    #[test]
    fn a_file_without_a_lookup_function_is_rejected_at_load() {
        assert!(matches!(
            load("nolookup", "x = 1\n"),
            Err(PythonError::NoLookup { .. })
        ));
    }

    #[test]
    fn a_syntax_error_is_reported_at_load_rather_than_on_the_first_stroke() {
        assert!(matches!(
            load("broken", "def lookup( this is not python\n"),
            Err(PythonError::Python { .. })
        ));
    }

    /// One broken dictionary costs the stroke, not the session.
    #[test]
    fn an_unexpected_exception_is_a_miss_rather_than_a_crash() {
        let dictionary = load(
            "raises",
            "LONGEST_KEY = 1\ndef lookup(key):\n    raise ValueError('deliberately broken')\n",
        )
        .expect("load");

        assert_eq!(dictionary.lookup(&outlines(&["X"])), None);
        assert_eq!(dictionary.lookup(&outlines(&["Y"])), None, "still usable");
    }

    #[test]
    fn reverse_lookup_is_optional_and_used_when_present() {
        let dictionary = load("noreverse", "def lookup(key):\n    raise KeyError\n").expect("load");
        assert!(dictionary.reverse_lookup("cat").is_empty());

        let dictionary = load(
            "reverse",
            "def lookup(key):\n    raise KeyError\ndef reverse_lookup(text):\n    return ['KAT'] if text == 'cat' else []\n",
        )
        .expect("load");
        assert_eq!(dictionary.reverse_lookup("cat"), vec!["KAT"]);
        assert!(dictionary.reverse_lookup("dog").is_empty());
    }

    #[test]
    fn the_module_body_runs_once_rather_than_per_lookup() {
        let dictionary = load(
            "sideeffect",
            "count = 0\ncount += 1\nLONGEST_KEY = 1\ndef lookup(key):\n    return str(count)\n",
        )
        .expect("load");

        assert_eq!(dictionary.lookup(&outlines(&["A"])), Some("1".to_owned()));
        assert_eq!(
            dictionary.lookup(&outlines(&["A"])),
            Some("1".to_owned()),
            "still 1, so the module was not re-executed"
        );
    }
}

/// Checks against the user's real `jeff-phrasing.py`.
///
/// Ignored by default because it depends on files outside this repository. Run
/// with `cargo test -p pluvialis-python -- --ignored --nocapture` when changing
/// anything about how Python dictionaries are called.
#[cfg(test)]
mod real_dictionary {
    use super::*;

    const DICTIONARY: &str = r"F:\Steno\jeff-phrasing.py";

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("phrasing_fixture.json")
    }

    /// Every answer, and every refusal, must match what Python produces.
    ///
    /// The fixture was generated from the same file by running it directly, so
    /// this checks the whole calling convention end to end: tuple packing,
    /// return extraction, and KeyError meaning "no entry".
    #[test]
    #[ignore = "depends on files outside the repository"]
    fn it_matches_python_on_every_enumerated_outline() {
        let dictionary = PythonDictionary::load(DICTIONARY).expect("loading jeff-phrasing.py");
        assert_eq!(dictionary.longest_key(), 1);

        let text = std::fs::read_to_string(fixture_path()).expect("reading the fixture");
        let expected: std::collections::HashMap<String, Option<String>> =
            serde_json::from_str(&text).expect("parsing the fixture");

        let mut checked = 0usize;
        let mut answered = 0usize;
        for (outline, want) in &expected {
            let got = dictionary.lookup(std::slice::from_ref(outline));
            assert_eq!(&got, want, "outline {outline:?}");
            checked += 1;
            answered += usize::from(want.is_some());
        }

        println!("checked {checked} outlines, {answered} answered");
        assert!(checked > 200_000, "fixture looks truncated: {checked}");
    }
}
