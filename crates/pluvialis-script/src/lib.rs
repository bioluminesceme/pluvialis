//! Lua host for programmatic ("scripty") dictionaries.
//!
//! A `.lua` dictionary exposes `lookup(strokes) -> string | nil` and optionally
//! `reverse_lookup(text) -> table of outline strings`. `strokes` arrives as a
//! table of outline strings, one per stroke, so a script sees the same thing a
//! JSON key would have matched.
//!
//! Only consulted when the dictionaries above it in priority order miss, so its
//! cost is near zero in normal writing.
//!
//! **Scripts are sandboxed.** A dictionary is data the user downloaded from
//! someone else, so it gets no filesystem, no network, no subprocesses and no
//! ability to load more Lua. It also gets a bounded number of VM instructions
//! per call, because the sandbox stops a script stealing data but not a script
//! looping forever, and a dictionary that hangs takes the writer down with it.

use std::path::{Path, PathBuf};

use mlua::{Lua, LuaOptions, StdLib, Value};

/// How many VM instructions one lookup may take before it is killed.
///
/// A lookup is a table walk and some string work. Ten million is far beyond any
/// honest dictionary and still trips in well under a second, so an accidental
/// infinite loop surfaces as one failed lookup rather than a frozen program.
const INSTRUCTION_BUDGET: u32 = 10_000_000;

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path}: {source}")]
    Lua {
        path: PathBuf,
        #[source]
        source: mlua::Error,
    },

    #[error("{path}: no lookup function, so this cannot act as a dictionary")]
    NoLookup { path: PathBuf },
}

/// A loaded Lua dictionary.
pub struct ScriptDictionary {
    lua: Lua,
    path: PathBuf,
    longest_key: usize,
    has_reverse: bool,
}

impl ScriptDictionary {
    /// Load and initialise a script.
    ///
    /// The script body runs once, here, so a syntax error or a broken table is
    /// reported at load time rather than on the first stroke that reaches it.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ScriptError> {
        let path = path.as_ref().to_path_buf();

        // UTF-8, like every other file this project reads.
        let source = std::fs::read_to_string(&path).map_err(|source| ScriptError::Io {
            path: path.clone(),
            source,
        })?;

        // The safe subset: string, table, math and friends. No io, no os, no
        // package, so a dictionary cannot read files, start processes or pull
        // in more code.
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            LuaOptions::default(),
        )
        .map_err(|source| ScriptError::Lua {
            path: path.clone(),
            source,
        })?;

        let globals = lua.globals();

        // Choosing the standard libraries is not enough. mlua always loads the
        // base library, and that carries `dofile` and `loadfile`, both of which
        // read files from disk: the exact hole excluding `io` was meant to
        // close. `load` and `loadstring` compile arbitrary code, and `require`
        // pulls in modules. None of them belong in a dictionary, so they are
        // removed explicitly before the script body runs.
        //
        // Verified by test rather than assumed. The first version of this
        // sandbox looked correct and left both file readers reachable.
        for name in [
            "dofile",
            "loadfile",
            "load",
            "loadstring",
            "require",
            "collectgarbage",
        ] {
            globals
                .set(name, Value::Nil)
                .map_err(|source| ScriptError::Lua {
                    path: path.clone(),
                    source,
                })?;
        }

        lua.load(&source)
            .set_name(path.to_string_lossy())
            .exec()
            .map_err(|source| ScriptError::Lua {
                path: path.clone(),
                source,
            })?;
        if !matches!(globals.get::<Value>("lookup"), Ok(Value::Function(_))) {
            return Err(ScriptError::NoLookup { path });
        }
        let has_reverse = matches!(
            globals.get::<Value>("reverse_lookup"),
            Ok(Value::Function(_))
        );

        // How far back the translator must look for this dictionary. Scripts
        // that do not say assume one stroke, which is what a phrasing style
        // dictionary usually wants.
        let longest_key = globals
            .get::<Option<usize>>("LONGEST_KEY")
            .ok()
            .flatten()
            .unwrap_or(1)
            .max(1);

        drop(globals);

        Ok(ScriptDictionary {
            lua,
            path,
            longest_key,
            has_reverse,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn longest_key(&self) -> usize {
        self.longest_key
    }

    /// Run `f` with the instruction budget armed, then disarm it.
    fn bounded<T>(&self, f: impl FnOnce() -> mlua::Result<T>) -> mlua::Result<T> {
        // A hook that will not install means no timeout, which is worse than a
        // failed lookup, so refuse to run rather than run unbounded.
        self.lua.set_hook(
            mlua::HookTriggers::new().every_nth_instruction(INSTRUCTION_BUDGET),
            move |_lua, _debug| {
                Err(mlua::Error::RuntimeError(
                    "dictionary script ran too long and was stopped".to_owned(),
                ))
            },
        )?;

        let result = f();
        self.lua.remove_hook();
        result
    }

    /// Look up one outline, e.g. `["KAT"]` or `["TPHOEU", "TPHOEU"]`.
    ///
    /// A script that errors is reported and treated as a miss: one broken
    /// dictionary should cost the stroke it was consulted for, not the session.
    pub fn lookup(&self, outlines: &[String]) -> Option<String> {
        let result = self.bounded(|| {
            let strokes = self.lua.create_sequence_from(outlines.iter().cloned())?;
            let function: mlua::Function = self.lua.globals().get("lookup")?;
            function.call::<Option<String>>(strokes)
        });

        match result {
            Ok(value) => value,
            Err(e) => {
                log::warn!("{}: lookup failed: {e}", self.path.display());
                None
            }
        }
    }

    /// Outlines that produce `text`, if the script offers a reverse lookup.
    pub fn reverse_lookup(&self, text: &str) -> Vec<String> {
        if !self.has_reverse {
            return Vec::new();
        }

        let result = self.bounded(|| {
            let function: mlua::Function = self.lua.globals().get("reverse_lookup")?;
            function.call::<Option<Vec<String>>>(text.to_owned())
        });

        match result {
            Ok(value) => value.unwrap_or_default(),
            Err(e) => {
                log::warn!("{}: reverse_lookup failed: {e}", self.path.display());
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a script to a temp file and load it.
    fn script(name: &str, body: &str) -> Result<ScriptDictionary, ScriptError> {
        let path = std::env::temp_dir().join(format!("pluvialis-{name}.lua"));
        std::fs::write(&path, body).expect("writing the test script");
        ScriptDictionary::load(&path)
    }

    fn outlines(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_script_answers_a_lookup() {
        let dictionary = script(
            "basic",
            r#"
            function lookup(strokes)
                if #strokes == 1 and strokes[1] == "KAT" then return "cat" end
                return nil
            end
            "#,
        )
        .expect("load");

        assert_eq!(
            dictionary.lookup(&outlines(&["KAT"])),
            Some("cat".to_owned())
        );
        assert_eq!(dictionary.lookup(&outlines(&["TKOG"])), None);
    }

    #[test]
    fn a_multi_stroke_outline_arrives_in_order() {
        let dictionary = script(
            "multi",
            r#"
            function lookup(strokes)
                return table.concat(strokes, "/")
            end
            "#,
        )
        .expect("load");

        assert_eq!(
            dictionary.lookup(&outlines(&["A", "B", "C"])),
            Some("A/B/C".to_owned())
        );
    }

    #[test]
    fn longest_key_defaults_to_one_and_is_read_when_given() {
        let dictionary = script("nokey", "function lookup(s) return nil end").expect("load");
        assert_eq!(dictionary.longest_key(), 1);

        let dictionary =
            script("withkey", "LONGEST_KEY = 4\nfunction lookup(s) return nil end").expect("load");
        assert_eq!(dictionary.longest_key(), 4);
    }

    #[test]
    fn a_script_without_a_lookup_function_is_rejected_at_load() {
        assert!(matches!(
            script("nolookup", "x = 1"),
            Err(ScriptError::NoLookup { .. })
        ));
    }

    #[test]
    fn a_syntax_error_is_reported_at_load_rather_than_on_the_first_stroke() {
        assert!(matches!(
            script("broken", "function lookup( this is not lua"),
            Err(ScriptError::Lua { .. })
        ));
    }

    /// A dictionary is somebody else's code. It must not be able to read files.
    #[test]
    fn the_filesystem_is_not_reachable_from_a_script() {
        let dictionary = script(
            "sandbox_io",
            r#"
            function lookup(strokes)
                if io == nil then return "no io" end
                return "io exists"
            end
            "#,
        )
        .expect("load");

        assert_eq!(
            dictionary.lookup(&outlines(&["X"])),
            Some("no io".to_owned())
        );
    }

    #[test]
    fn os_and_package_are_not_reachable_either() {
        let dictionary = script(
            "sandbox_os",
            r#"
            function lookup(strokes)
                local names = {}
                if os == nil then names[#names+1] = "no os" end
                if package == nil then names[#names+1] = "no package" end
                if dofile == nil then names[#names+1] = "no dofile" end
                if loadfile == nil then names[#names+1] = "no loadfile" end
                if load == nil then names[#names+1] = "no load" end
                if require == nil then names[#names+1] = "no require" end
                return table.concat(names, ",")
            end
            "#,
        )
        .expect("load");

        assert_eq!(
            dictionary.lookup(&outlines(&["X"])),
            Some("no os,no package,no dofile,no loadfile,no load,no require".to_owned())
        );
    }

    /// The specific hole the first version of this sandbox left open: choosing
    /// standard libraries excludes `io`, but the base library still carries
    /// `dofile` and `loadfile`, and both read files from disk.
    #[test]
    fn a_script_cannot_read_a_file_through_the_base_library() {
        let secret = std::env::temp_dir().join("pluvialis-should-not-be-readable.lua");
        std::fs::write(&secret, "leaked = 'secret contents'").expect("writing the bait");

        let dictionary = script(
            "escape",
            &format!(
                r#"
                function lookup(strokes)
                    if dofile == nil then return "blocked" end
                    local ok = pcall(dofile, [[{}]])
                    if ok then return "LEAKED" end
                    return "blocked"
                end
                "#,
                secret.display().to_string().replace('\\', "\\\\")
            ),
        )
        .expect("load");

        assert_eq!(
            dictionary.lookup(&outlines(&["X"])),
            Some("blocked".to_owned())
        );
        let _ = std::fs::remove_file(&secret);
    }

    /// The sandbox stops a script stealing data; it does not stop one looping
    /// forever, and a dictionary that hangs takes the writer down with it.
    #[test]
    fn a_script_that_loops_forever_is_stopped_rather_than_hanging() {
        let dictionary = script(
            "infinite",
            r#"
            function lookup(strokes)
                while true do end
            end
            "#,
        )
        .expect("load");

        let started = std::time::Instant::now();
        assert_eq!(
            dictionary.lookup(&outlines(&["X"])),
            None,
            "treated as a miss"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "took {:?}, the instruction budget did not fire",
            started.elapsed()
        );
    }

    /// One broken dictionary costs the stroke, not the session.
    #[test]
    fn a_script_that_errors_is_a_miss_rather_than_a_crash() {
        let dictionary = script(
            "raises",
            r#"
            function lookup(strokes)
                error("deliberately broken")
            end
            "#,
        )
        .expect("load");

        assert_eq!(dictionary.lookup(&outlines(&["X"])), None);
        // Still usable afterwards.
        assert_eq!(dictionary.lookup(&outlines(&["Y"])), None);
    }

    #[test]
    fn reverse_lookup_is_optional_and_used_when_present() {
        let dictionary = script("noreverse", "function lookup(s) return nil end").expect("load");
        assert!(dictionary.reverse_lookup("cat").is_empty());

        let dictionary = script(
            "reverse",
            r#"
            function lookup(s) return nil end
            function reverse_lookup(text)
                if text == "cat" then return {"KAT", "KAT/KAT"} end
                return {}
            end
            "#,
        )
        .expect("load");

        assert_eq!(dictionary.reverse_lookup("cat"), vec!["KAT", "KAT/KAT"]);
        assert!(dictionary.reverse_lookup("dog").is_empty());
    }
}
