//! Scalar source-to-LLVM emission. Generated operations are native, never interpreted.
use crate::{
    Diagnostic,
    effects::{self, Effect},
    flow,
    ir::{Binary, BranchMode, Operation, Terminator, Unary},
    parser::{Ast, Syntax},
    ranges,
    sema::Scalar,
};
use std::fmt::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    MacX86_64,
    MacArm64,
    LinuxX86_64,
    WindowsX86_64,
}
impl Target {
    pub fn host() -> Result<Self, String> {
        Self::for_host(std::env::consts::OS, std::env::consts::ARCH)
    }
    pub fn for_host(os: &str, arch: &str) -> Result<Self, String> {
        match (os, arch) {
            ("macos", "x86_64") => Ok(Self::MacX86_64),
            ("macos", "aarch64") => Ok(Self::MacArm64),
            ("linux", "x86_64") => Ok(Self::LinuxX86_64),
            ("windows", "x86_64") => Ok(Self::WindowsX86_64),
            _ => Err(format!("unsupported native host: {os}/{arch}")),
        }
    }
    pub fn is_macos(self) -> bool {
        matches!(self, Self::MacX86_64 | Self::MacArm64)
    }
    pub fn is_windows(self) -> bool {
        self == Self::WindowsX86_64
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::MacX86_64 => "macos-x86_64",
            Self::MacArm64 => "macos-arm64",
            Self::LinuxX86_64 => "linux-x86_64",
            Self::WindowsX86_64 => "windows-x86_64",
        }
    }
    pub fn arch(self) -> &'static str {
        if self == Self::MacArm64 {
            "arm64"
        } else {
            "x86_64"
        }
    }
    pub fn rust_triple(self) -> &'static str {
        match self {
            Self::MacX86_64 => "x86_64-apple-darwin",
            Self::MacArm64 => "aarch64-apple-darwin",
            _ => self.triple(),
        }
    }
    pub fn slices(self) -> &'static [Self] {
        match self {
            Self::MacX86_64 | Self::MacArm64 => &[Self::MacX86_64, Self::MacArm64],
            Self::LinuxX86_64 => &[Self::LinuxX86_64],
            Self::WindowsX86_64 => &[Self::WindowsX86_64],
        }
    }
    pub fn bundle_name(self) -> &'static str {
        if self.is_macos() {
            "macos-universal"
        } else {
            self.name()
        }
    }
    pub fn shared_ext(self) -> &'static str {
        if self.is_macos() {
            "dylib"
        } else if self.is_windows() {
            "dll"
        } else {
            "so"
        }
    }
    pub fn object_ext(self) -> &'static str {
        if self.is_windows() { "obj" } else { "o" }
    }
    pub fn archive_ext(self) -> &'static str {
        if self.is_windows() { "lib" } else { "a" }
    }
    pub fn executable(self, name: &str) -> String {
        if self.is_windows() {
            format!("{name}.exe")
        } else {
            name.into()
        }
    }
    pub fn ir_symbol(self, symbol: &str) -> &str {
        if self.is_macos() {
            symbol.strip_prefix('_').unwrap_or(symbol)
        } else {
            symbol
        }
    }
    pub fn cpu(self) -> &'static str {
        match self {
            Self::MacX86_64 | Self::LinuxX86_64 | Self::WindowsX86_64 => "x86-64",
            Self::MacArm64 => "generic",
        }
    }
    pub fn clang_cpu(self) -> &'static str {
        match self {
            Self::MacX86_64 | Self::LinuxX86_64 | Self::WindowsX86_64 => "-march=x86-64",
            Self::MacArm64 => "-mcpu=generic",
        }
    }
    pub fn triple(self) -> &'static str {
        match self {
            Self::MacX86_64 => "x86_64-apple-macosx14.0.0",
            Self::MacArm64 => "arm64-apple-macosx14.0.0",
            Self::LinuxX86_64 => "x86_64-unknown-linux-gnu",
            Self::WindowsX86_64 => "x86_64-pc-windows-msvc",
        }
    }
}
struct Text(String);
impl Write for Text {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.try_reserve(s.len()).map_err(|_| fmt::Error)?;
        self.0.push_str(s);
        Ok(())
    }
}
struct Emitter {
    optimize: bool,
    specialized: bool,
    word: &'static str,
    runtime: Option<String>,
    out: Text,
    next: usize,
    frame: Option<String>,
    exception: Option<crate::ir::ExceptionEdge>,
    /// The session id, read once in the entry block and reused.
    ///
    /// The thread-local session cannot change during a running function.
    /// Reusing the entry read avoids repeated opaque external calls.
    session: Option<String>,
    /// The label of this function's shared epilogue, when it has one.
    ///
    /// Every exit stores its outcome and jumps here, ensuring the frame is
    /// released exactly once without duplicating the release sequence.
    epilogue: Option<usize>,
}
impl Emitter {
    fn outcome(&self) -> &'static str {
        if self.specialized { "%iout" } else { "%out" }
    }
    fn owner_field(&self) -> &'static str {
        if self.word == "i128" && !self.specialized {
            ", i64 0"
        } else {
            ""
        }
    }
    fn data_type(&self) -> &'static str {
        if self.specialized { "i32" } else { self.word }
    }
    fn unbox_known(&mut self, value: &str) -> Result<String, Diagnostic> {
        let word = self.word;
        let bits = self.temp(format_args!("lshr {word} {value}, 32"))?;
        self.temp(format_args!("trunc {word} {bits} to i32"))
    }
    fn line(&mut self, args: fmt::Arguments<'_>) -> Result<(), Diagnostic> {
        self.out
            .write_fmt(args)
            .and_then(|()| self.out.write_char('\n'))
            .map_err(|_| Diagnostic::resource(0))
    }
    fn temp(&mut self, args: fmt::Arguments<'_>) -> Result<String, Diagnostic> {
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or(Diagnostic::resource(0))?;
        self.line(format_args!("  %t{id} = {args}"))?;
        Ok(format!("%t{id}"))
    }
    /// The session id for this function, read once if it has been hoisted.
    fn session_value(&mut self) -> Result<String, Diagnostic> {
        if let Some(session) = &self.session {
            return Ok(session.clone());
        }
        self.temp(format_args!("call i64 @zeb_objects_session()"))
    }
    /// Read the session once, in the entry block, which dominates every other.
    fn hoist_session(&mut self) -> Result<(), Diagnostic> {
        let session = self.temp(format_args!("call i64 @zeb_objects_session()"))?;
        self.session = Some(session);
        Ok(())
    }
    /// The frame release and the return, written once at the end of a function.
    ///
    /// Every `exit` stored its outcome and jumped here. The frame handle and
    /// the session are entry-block values, so both dominate this block however
    /// it was reached.
    fn write_epilogue(&mut self) -> Result<(), Diagnostic> {
        let Some(label) = self.epilogue.take() else {
            return Ok(());
        };
        let frame = self.frame.clone().expect("an epilogue releases a frame");
        let outcome = self.outcome();
        let data = self.data_type();
        let owner = self.owner_field();
        self.line(format_args!("epilogue{label}:"))?;
        let result = self.temp(format_args!("load {outcome}, ptr %ret_out"))?;
        let session = self.session_value()?;
        self.temp(format_args!(
            "call i64 @zeb_objects_call(i32 65, i64 {session}, i64 {frame}, i64 0, i64 0)"
        ))?;
        let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
        let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
        let k = self.next;
        self.next = self.next.checked_add(1).ok_or(Diagnostic::resource(0))?;
        self.line(format_args!(
            "  br i1 {bad}, label %cleanup_error{k}, label %cleanup_done{k}\ncleanup_error{k}:\n  ret {outcome} {{ {data} 0, i32 7, i64 0{owner} }}\ncleanup_done{k}:\n  ret {outcome} {result}"
        ))
    }
    fn exit(&mut self, result: &str) -> Result<(), Diagnostic> {
        if let Some(label) = self.epilogue {
            let outcome = self.outcome();
            self.line(format_args!(
                "  store {outcome} {result}, ptr %ret_out\n  br label %epilogue{label}"
            ))?;
            return Ok(());
        }
        let outcome = self.outcome();
        let data = self.data_type();
        let owner = self.owner_field();
        if let Some(frame) = self.frame.clone() {
            let session = self.session_value()?;
            self.temp(format_args!(
                "call i64 @zeb_objects_call(i32 65, i64 {session}, i64 {frame}, i64 0, i64 0)"
            ))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            let label = self.next;
            self.next += 1;
            self.line(format_args!("  br i1 {bad}, label %cleanup_error{label}, label %cleanup_done{label}\ncleanup_error{label}:\n  ret {outcome} {{ {data} 0, i32 7, i64 0{owner} }}\ncleanup_done{label}:"))?;
        }
        self.line(format_args!("  ret {outcome} {result}"))
    }
    fn propagate(
        &mut self,
        result: &str,
        edge: Option<crate::ir::ExceptionEdge>,
    ) -> Result<(), Diagnostic> {
        if let Some(edge) = edge {
            let code = self.temp(format_args!("extractvalue %out {result}, 1"))?;
            let source = self.temp(format_args!("icmp eq i32 {code}, 9"))?;
            let label = self.next;
            self.next += 1;
            self.line(format_args!(
                "  br i1 {source}, label %caught{label}, label %uncaught{label}\ncaught{label}:"
            ))?;
            self.line(format_args!("  store %out {result}, ptr %pending_out"))?;
            let value = self.temp(format_args!("extractvalue %out {result}, 0"))?;
            self.line(format_args!(
                "  store i128 {value}, ptr %s{}, align 8\n  br label %b{}\nuncaught{label}:",
                edge.slot.0, edge.target.0
            ))?;
        }
        self.exit(result)
    }
    fn guard(&mut self, bad: &str, code: u32, site: usize) -> Result<(), Diagnostic> {
        let label = self.next;
        self.next = self.next.checked_add(1).ok_or(Diagnostic::resource(site))?;
        let data = self.data_type();
        self.line(format_args!(
            "  br i1 {bad}, label %err{label}, label %ok{label}\nerr{label}:"
        ))?;
        let owner = self.owner_field();
        self.exit(&format!("{{ {data} 0, i32 {code}, i64 {site}{owner} }}"))?;
        self.line(format_args!("ok{label}:"))
    }
    fn validate_runtime(&mut self) -> Result<(), Diagnostic> {
        let symbol = self.runtime.as_ref().expect("runtime mode").clone();
        let version = self.temp(format_args!("load i32, ptr @{symbol}"))?;
        let wrong_version = self.temp(format_args!("icmp ne i32 {version}, 1"))?;
        self.guard(&wrong_version, 5, 0)?;
        let size_address = self.temp(format_args!(
            "getelementptr %scalar_api, ptr @{symbol}, i32 0, i32 1"
        ))?;
        let size = self.temp(format_args!("load i32, ptr {size_address}"))?;
        let wrong_size = self.temp(format_args!("icmp ne i32 {size}, 16"))?;
        self.guard(&wrong_size, 5, 0)
    }
    fn object_call(
        &mut self,
        op: &str,
        a: &str,
        b: &str,
        c: &str,
        site: usize,
    ) -> Result<String, Diagnostic> {
        let session = self.session_value()?;
        let result = self.temp(format_args!(
            "call i64 @zeb_objects_call(i32 {op}, i64 {session}, i64 {a}, i64 {b}, i64 {c})"
        ))?;
        let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
        let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
        self.guard(&bad, 7, site)?;
        Ok(result)
    }
    /// Split a value into its kind tag and payload, both as i64.
    fn tagged(&mut self, value: &str) -> Result<(String, String), Diagnostic> {
        let tag = self.temp(format_args!("trunc i128 {value} to i32"))?;
        let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
        let bits = self.temp(format_args!("lshr i128 {value}, 32"))?;
        let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
        Ok((tag, payload))
    }
    fn pack_text(&mut self, handle: &str) -> Result<String, Diagnostic> {
        let wide = self.temp(format_args!("zext i64 {handle} to i128"))?;
        let bits = self.temp(format_args!("shl i128 {wide}, 32"))?;
        self.temp(format_args!("or i128 {bits}, 7"))
    }
    fn text_handle(&mut self, value: &str, site: usize) -> Result<String, Diagnostic> {
        let tag = self.temp(format_args!("trunc i128 {value} to i32"))?;
        let literal = self.temp(format_args!("icmp eq i32 {tag}, 4"))?;
        let owned = self.temp(format_args!("icmp eq i32 {tag}, 7"))?;
        let valid = self.temp(format_args!("or i1 {literal}, {owned}"))?;
        let bad = self.temp(format_args!("xor i1 {valid}, true"))?;
        self.guard(&bad, 1, site)?;
        let op = self.temp(format_args!("select i1 {literal}, i32 21, i32 31"))?;
        let bits = self.temp(format_args!("lshr i128 {value}, 32"))?;
        let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
        self.object_call(&op, &payload, "0", "0", site)
    }
    fn string_builtin(
        &mut self,
        kind: crate::sema::Builtin,
        arguments: &[crate::ir::Value],
        site: usize,
    ) -> Result<String, Diagnostic> {
        use crate::sema::Builtin;
        if self.word != "i128" {
            return Err(Diagnostic::new(
                "native-profile",
                "string intrinsics require the wide object profile",
                site,
            ));
        }
        let arg = |n: usize| format!("%v{}", arguments[n].0);
        if kind == Builtin::NewLocalLookupSized {
            let buckets = self.integer(&arg(0), site, false)?;
            let capacity = self.integer(&arg(1), site, false)?;
            let buckets = self.temp(format_args!("zext i32 {buckets} to i64"))?;
            let capacity = self.temp(format_args!("zext i32 {capacity} to i64"))?;
            let table = self.object_call("97", &buckets, &capacity, "0", site)?;
            let bits = self.temp(format_args!("zext i64 {table} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 3"));
        }
        if matches!(
            kind,
            Builtin::NewLocalLookup
                | Builtin::NewLocalStringBuffer
                | Builtin::NewLocalOwnedCollection
        ) {
            let handle = self.object_call(
                if kind == Builtin::NewLocalLookup {
                    "85"
                } else if kind == Builtin::NewLocalOwnedCollection {
                    "98"
                } else {
                    "86"
                },
                "0",
                "0",
                "0",
                site,
            )?;
            let bits = self.temp(format_args!("zext i64 {handle} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 3"));
        }
        if matches!(
            kind,
            Builtin::ListIndex
                | Builtin::LookupSet
                | Builtin::IndexSet
                | Builtin::LookupDefault
                | Builtin::LookupSetDefault
                | Builtin::LookupBuckets
                | Builtin::LookupLength
                | Builtin::LookupContains
        ) {
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let valid = self.temp(format_args!("icmp eq i32 {tag}, 3"))?;
            let valid = if kind == Builtin::ListIndex {
                let list = self.temp(format_args!("icmp eq i32 {tag}, 9"))?;
                self.temp(format_args!("or i1 {valid}, {list}"))?
            } else {
                valid
            };
            let bad = self.temp(format_args!("xor i1 {valid}, true"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let table = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let key = if matches!(
                kind,
                Builtin::ListIndex
                    | Builtin::LookupSet
                    | Builtin::IndexSet
                    | Builtin::LookupContains
            ) {
                arg(1)
            } else {
                "0".to_owned()
            };
            let value = match kind {
                Builtin::LookupSet | Builtin::IndexSet => arg(2),
                Builtin::LookupSetDefault => arg(1),
                _ => "0".to_owned(),
            };
            let key_tag = self.temp(format_args!("trunc i128 {key} to i32"))?;
            let key_tag = self.temp(format_args!("zext i32 {key_tag} to i64"))?;
            let value_tag = self.temp(format_args!("trunc i128 {value} to i32"))?;
            let value_tag = self.temp(format_args!("zext i32 {value_tag} to i64"))?;
            let value_tag = self.temp(format_args!("shl i64 {value_tag}, 32"))?;
            let flags = self.temp(format_args!("or i64 {value_tag}, {key_tag}"))?;
            let key = self.temp(format_args!("lshr i128 {key}, 32"))?;
            let key = self.temp(format_args!("trunc i128 {key} to i64"))?;
            let value = self.temp(format_args!("lshr i128 {value}, 32"))?;
            let value = self.temp(format_args!("trunc i128 {value} to i64"))?;
            let operation = match kind {
                Builtin::ListIndex => 6,
                Builtin::LookupSet => 1,
                Builtin::IndexSet => 7,
                Builtin::LookupContains => 2,
                Builtin::LookupLength => 3,
                Builtin::LookupBuckets => 8,
                Builtin::LookupDefault => 4,
                _ => 5,
            };
            let session = self.session_value()?;
            let payload = self.temp(format_args!("call i64 @zeb_objects_lookup_call(i32 {operation}, i64 {session}, i64 {table}, i64 {flags}, i64 {key}, i64 {value})"))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            self.guard(&bad, 7, site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let payload = self.temp(format_args!("zext i64 {payload} to i128"))?;
            let payload = self.temp(format_args!("shl i128 {payload}, 32"))?;
            return self.temp(format_args!("or i128 {payload}, {tag}"));
        }
        if kind == Builtin::RexReplace {
            // pattern, subject and replacement text, plus the
            // reference's flags; the replaced text is owned by this scope.
            let mut parts = Vec::new();
            parts
                .try_reserve(6)
                .map_err(|_| Diagnostic::resource(site))?;
            for index in [0usize, 1, 2] {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let literal = self.temp(format_args!("icmp eq i32 {tag}, 4"))?;
                let owned = self.temp(format_args!("icmp eq i32 {tag}, 7"))?;
                let valid = self.temp(format_args!("or i1 {literal}, {owned}"))?;
                let valid = if index == 0 {
                    let object = self.temp(format_args!("icmp eq i32 {tag}, 3"))?;
                    self.temp(format_args!("or i1 {valid}, {object}"))?
                } else {
                    valid
                };
                let bad = self.temp(format_args!("xor i1 {valid}, true"))?;
                self.guard(&bad, 1, site)?;
                let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                parts.push(tag);
                parts.push(payload);
            }
            let flags = if arguments.len() == 4 {
                let value = self.integer(&arg(3), site, false)?;
                self.temp(format_args!("zext i32 {value} to i64"))?
            } else {
                "1".to_owned() // ReplaceAll is the reference default
            };
            // subject carries its flags, and the index word the replacement.
            let subject = self.temp(format_args!("shl i64 {flags}, 32"))?;
            let subject = self.temp(format_args!("or i64 {subject}, {}", parts[3]))?;
            let replacement = self.temp(format_args!("shl i64 {}, 32", parts[4]))?;
            let replacement = self.temp(format_args!("or i64 {replacement}, {}", parts[5]))?;
            let session = self.session_value()?;
            let result = self.temp(format_args!(
                "call i64 @zeb_objects_regex_call(i32 7, i64 {session}, i64 {}, i64 {}, i64 {}, i64 {subject}, i64 {replacement})",
                parts[0], parts[1], parts[2]
            ))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            self.guard(&bad, 7, site)?;
            return self.pack_text(&result);
        }
        if kind == Builtin::RexGroup {
            // the group's [start, length, text], owned by this scope.
            let group = self.integer(&arg(0), site, false)?;
            let group = self.temp(format_args!("zext i32 {group} to i64"))?;
            let session = self.session_value()?;
            let payload = self.temp(format_args!(
                "call i64 @zeb_objects_regex_call(i32 5, i64 {session}, i64 0, i64 0, i64 0, i64 0, i64 {group})"
            ))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            self.guard(&bad, 7, site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let payload = self.temp(format_args!("zext i64 {payload} to i128"))?;
            let payload = self.temp(format_args!("shl i128 {payload}, 32"))?;
            return self.temp(format_args!("or i128 {payload}, {tag}"));
        }
        if matches!(kind, Builtin::RexMatch | Builtin::RexSearch) {
            // pattern and subject are literal or owned text; a source
            // index is one-based, and 0 means "from the start".
            let mut parts = Vec::new();
            parts
                .try_reserve(4)
                .map_err(|_| Diagnostic::resource(site))?;
            for index in 0..2 {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let literal = self.temp(format_args!("icmp eq i32 {tag}, 4"))?;
                let owned = self.temp(format_args!("icmp eq i32 {tag}, 7"))?;
                let valid = self.temp(format_args!("or i1 {literal}, {owned}"))?;
                // the pattern may be a compiled RexPattern object
                let valid = if index == 0 {
                    let object = self.temp(format_args!("icmp eq i32 {tag}, 3"))?;
                    self.temp(format_args!("or i1 {valid}, {object}"))?
                } else {
                    valid
                };
                let bad = self.temp(format_args!("xor i1 {valid}, true"))?;
                self.guard(&bad, 1, site)?;
                let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                parts.push(tag);
                parts.push(payload);
            }
            let start = if arguments.len() == 3 {
                let value = self.integer(&arg(2), site, false)?;
                self.temp(format_args!("zext i32 {value} to i64"))?
            } else {
                "0".to_owned()
            };
            let session = self.session_value()?;
            let operation = if kind == Builtin::RexMatch { 0 } else { 4 };
            let payload = self.temp(format_args!(
                "call i64 @zeb_objects_regex_call(i32 {operation}, i64 {session}, i64 {}, i64 {}, i64 {}, i64 {}, i64 {start})",
                parts[0], parts[1], parts[2], parts[3]
            ))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            self.guard(&bad, 7, site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let payload = self.temp(format_args!("zext i64 {payload} to i128"))?;
            let payload = self.temp(format_args!("shl i128 {payload}, 32"))?;
            return self.temp(format_args!("or i128 {payload}, {tag}"));
        }
        if kind == Builtin::GrammarFinish {
            // assign properties to the innermost pending parse.
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 9"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let order = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let session = self.session_value()?;
            let payload = self.temp(format_args!(
                "call i64 @zeb_objects_grammar_call(i32 4, i64 {session}, i64 {order}, i64 0, i64 0, i64 0)"
            ))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            self.guard(&bad, 7, site)?;
            let bits = self.temp(format_args!("zext i64 {payload} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 9"));
        }
        if kind == Builtin::GrammarBegin {
            // Receiver production, token list and recipient collection are checked
            // natively; the dictionary may be nil.
            let mut payloads = Vec::new();
            payloads
                .try_reserve(3)
                .map_err(|_| Diagnostic::resource(site))?;
            for (index, expected) in [(0, 3), (1, 9), (3, 3)] {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let wrong = self.temp(format_args!("icmp ne i32 {tag}, {expected}"))?;
                let bad = if index == 3 {
                    // no recipient means the match trees belong to the
                    // turn, so nil is a valid answer here.
                    let nil = self.temp(format_args!("icmp eq i32 {tag}, 0"))?;
                    let absent = self.temp(format_args!("xor i1 {nil}, true"))?;
                    self.temp(format_args!("and i1 {wrong}, {absent}"))?
                } else {
                    wrong
                };
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                payloads.push(self.temp(format_args!("trunc i128 {bits} to i64"))?);
            }
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(2)))?;
            let nil = self.temp(format_args!("icmp eq i32 {tag}, 0"))?;
            let object = self.temp(format_args!("icmp eq i32 {tag}, 3"))?;
            let valid = self.temp(format_args!("or i1 {nil}, {object}"))?;
            let bad = self.temp(format_args!("xor i1 {valid}, true"))?;
            self.guard(&bad, 1, site)?;
            let operation = self.temp(format_args!("zext i1 {nil} to i32"))?;
            let operation = self.temp(format_args!("add i32 {operation}, 2"))?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(2)))?;
            let dictionary = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let session = self.session_value()?;
            let payload = self.temp(format_args!(
                "call i64 @zeb_objects_grammar_call(i32 {operation}, i64 {session}, i64 {}, i64 {}, i64 {dictionary}, i64 {})",
                payloads[0], payloads[1], payloads[2]
            ))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            self.guard(&bad, 7, site)?;
            let bits = self.temp(format_args!("zext i64 {payload} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 9"));
        }
        if matches!(
            kind,
            Builtin::DictionaryAdd
                | Builtin::DictionaryRemove
                | Builtin::DictionaryFind
                | Builtin::DictionaryDefined
                | Builtin::VocabularyWords
                | Builtin::VocabularyNames
        ) {
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let dictionary = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            // the vocabulary-relation operations name an entity in the
            // same operand `addWord` does, so they share the argument shape:
            // dictionary, entity, then the word where there is one, then the
            // vocabulary property.
            let mutation = matches!(
                kind,
                Builtin::DictionaryAdd
                    | Builtin::DictionaryRemove
                    | Builtin::VocabularyNames
                    | Builtin::VocabularyWords
            );
            let target = if mutation {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(1)))?;
                let bad = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(1)))?;
                self.temp(format_args!("trunc i128 {bits} to i64"))?
            } else {
                "0".to_owned()
            };
            // Reading the words that name an entity asks about no word at all.
            let (word_kind, word) = if kind == Builtin::VocabularyWords {
                ("0".to_owned(), "0".to_owned())
            } else {
                let word = arg(if mutation { 2 } else { 1 });
                let word_kind = self.temp(format_args!("trunc i128 {word} to i64"))?;
                let word_kind = self.temp(format_args!("and i64 {word_kind}, 4294967295"))?;
                let bits = self.temp(format_args!("lshr i128 {word}, 32"))?;
                (
                    word_kind,
                    self.temp(format_args!("trunc i128 {bits} to i64"))?,
                )
            };
            let property_index = if kind == Builtin::VocabularyWords {
                Some(2)
            } else if mutation {
                Some(3)
            } else if arguments.len() == 3 {
                Some(2)
            } else {
                None
            };
            let (property_kind, property) = if let Some(index) = property_index {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                (tag, self.temp(format_args!("trunc i128 {bits} to i64"))?)
            } else {
                ("0".to_owned(), "0".to_owned())
            };
            let shifted = self.temp(format_args!("shl i64 {property_kind}, 32"))?;
            let flags = self.temp(format_args!("or i64 {shifted}, {word_kind}"))?;
            let session = self.session_value()?;
            let operation = match kind {
                Builtin::DictionaryAdd => 0,
                Builtin::DictionaryRemove => 1,
                Builtin::DictionaryFind => 2,
                Builtin::VocabularyWords => 4,
                Builtin::VocabularyNames => 5,
                _ => 3,
            };
            let payload = self.temp(format_args!("call i64 @zeb_objects_dictionary_call(i32 {operation}, i64 {session}, i64 {dictionary}, i64 {target}, i64 {flags}, i64 {word}, i64 {property})"))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            self.guard(&bad, 7, site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let bits = self.temp(format_args!("zext i64 {payload} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, {tag}"));
        }
        if matches!(kind, Builtin::ArgumentPack | Builtin::ArgumentPackTail) {
            if self.word != "i128" {
                return Err(Diagnostic::new(
                    "native-profile",
                    "argument expansion requires the object profile",
                    site,
                ));
            }
            // ArgumentPack copies fixed values then the list; ArgumentPackTail the list first.
            let leading = kind == Builtin::ArgumentPackTail;
            let (tail, fixed) = if leading {
                (arg(0), &arguments[1..])
            } else {
                (arg(arguments.len() - 1), &arguments[..arguments.len() - 1])
            };
            let tag = self.temp(format_args!("trunc i128 {tail} to i32"))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 9"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {tail}, 32"))?;
            let handle = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let length = self.object_call("39", &handle, "9", "0", site)?;
            let total = self.temp(format_args!("add i64 {length}, {}", arguments.len() - 1))?;
            let bad = self.temp(format_args!("icmp ugt i64 {total}, 2147483647"))?;
            self.guard(&bad, 7, site)?;
            let builder = self.object_call("73", &total, "0", "0", site)?;
            for list_now in [leading, !leading] {
                if !list_now {
                    for value in fixed {
                        let tag = self.temp(format_args!("trunc i128 %v{} to i32", value.0))?;
                        let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
                        let bits = self.temp(format_args!("lshr i128 %v{}, 32", value.0))?;
                        let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                        self.object_call("74", &builder, &tag, &payload, site)?;
                    }
                    continue;
                }
                let label = self.next;
                self.next = self.next.checked_add(1).ok_or(Diagnostic::resource(site))?;
                self.line(format_args!("  br label %packstart{label}\npackstart{label}:\n  br label %packtest{label}\npacktest{label}:\n  %packindex{label} = phi i64 [ 1, %packstart{label} ], [ %packinc{label}, %packnext{label} ]"))?;
                let more = self.temp(format_args!("icmp ule i64 %packindex{label}, {length}"))?;
                self.line(format_args!(
                "  br i1 {more}, label %packbody{label}, label %packdone{label}\npackbody{label}:"
            ))?;
                let payload =
                    self.object_call("40", &handle, &format!("%packindex{label}"), "0", site)?;
                let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
                let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
                self.object_call("74", &builder, &tag, &payload, site)?;
                self.line(format_args!("  br label %packnext{label}\npacknext{label}:\n  %packinc{label} = add i64 %packindex{label}, 1\n  br label %packtest{label}\npackdone{label}:"))?;
            }
            self.object_call("38", &builder, "0", "0", site)?;
            let bits = self.temp(format_args!("zext i64 {builder} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 9"));
        }
        if kind == Builtin::ConstantList {
            let index = self.integer(&arg(0), site, true)?;
            let index = self.temp(format_args!("zext i32 {index} to i64"))?;
            let handle = self.object_call("111", &index, "0", "0", site)?;
            let bits = self.temp(format_args!("zext i64 {handle} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 9"));
        }
        if kind == Builtin::List {
            let builder = self.object_call("36", &arguments.len().to_string(), "0", "0", site)?;
            for value in arguments {
                let kind = self.temp(format_args!("trunc i128 %v{} to i64", value.0))?;
                let kind = self.temp(format_args!("and i64 {kind}, 4294967295"))?;
                let bits = self.temp(format_args!("lshr i128 %v{}, 32", value.0))?;
                let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                self.object_call("37", &builder, &kind, &payload, site)?;
            }
            self.object_call("38", &builder, "0", "0", site)?;
            let bits = self.temp(format_args!("zext i64 {builder} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 9"));
        }
        if kind == Builtin::Length {
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let handle = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
            let length = self.object_call("39", &handle, &tag, "0", site)?;
            let length = self.temp(format_args!("trunc i64 {length} to i32"))?;
            return self.pack(&length);
        }
        if matches!(kind, Builtin::InputKey | Builtin::InputLine) {
            // console input as scope-owned text. inputLine may
            // report the end of input, so its kind tag decides the result.
            let op = if kind == Builtin::InputKey {
                "118"
            } else {
                "125"
            };
            let handle = self.object_call(op, "0", "0", "0", site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let bits = self.temp(format_args!("zext i64 {handle} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, {tag}"));
        }
        // /: relation tables and the turn boundary. The first
        // argument of a relation call is the packed descriptor the analysis
        // resolved from the name, so the runtime needs no lookup.
        if matches!(kind, Builtin::HostAction | Builtin::EngineQuery) {
            // the game proposes and waits. The host accepts an action
            // or answers a query, and only then does the turn carry on.
            let code = self.integer(&arg(0), site, false)?;
            let code = self.temp(format_args!("zext i32 {code} to i64"))?;
            let op = if kind == Builtin::HostAction {
                "143"
            } else {
                "144"
            };
            let value = self.object_call(op, &code, "0", "0", site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            if kind == Builtin::HostAction {
                // true or nil, which are the tag alone: a logical value carries
                // no payload, and one here would fail the condition guard.
                return self.temp(format_args!("zext i32 {tag} to i128"));
            }
            // An integer answer, or nil when the host closed instead.
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let bits = self.temp(format_args!("zext i64 {value} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, {tag}"));
        }
        // an event is an id and the entities it concerns. The id is a
        // value, so it costs a tag and a payload; an entity is a handle. Two
        // calls rather than one because the operation boundary has three
        // payload slots and this needs four.
        if kind == Builtin::Event || kind == Builtin::EventValue {
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let wide = self.temp(format_args!("zext i32 {tag} to i64"))?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            self.object_call(
                if kind == Builtin::Event { "146" } else { "152" },
                &wide,
                &payload,
                "0",
                site,
            )?;
            return Ok("0".to_owned());
        }
        if kind == Builtin::SnapshotToken {
            let n = self.integer(&arg(0), site, false)?;
            let n = self.temp(format_args!("zext i32 {n} to i64"))?;
            let value = self.object_call("153", &n, "0", "0", site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let bits = self.temp(format_args!("zext i64 {value} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, {tag}"));
        }
        if kind == Builtin::EventSubject {
            // Only an object can be an entity, the same guard despawn uses.
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let handle = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            self.object_call("147", &handle, "0", "0", site)?;
            return Ok("0".to_owned());
        }
        if kind == Builtin::Despawn {
            // the entity goes, and every relation row naming it with
            // it. The argument must be an object; nothing else can be an entity.
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let handle = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            self.object_call("141", &handle, "0", "0", site)?;
            return Ok("0".to_owned());
        }
        if matches!(
            kind,
            Builtin::BeginTurn
                | Builtin::Savepoint
                | Builtin::EndTurn
                | Builtin::TurnJournalLength
                | Builtin::UseLifetimes
                | Builtin::Undo
                | Builtin::UndoDepth
        ) {
            let selector = match kind {
                Builtin::BeginTurn => "0",
                Builtin::Savepoint => "9",
                Builtin::EndTurn => "1",
                Builtin::UseLifetimes => "3",
                Builtin::Undo => "5",
                Builtin::UndoDepth => "6",
                _ => "2",
            };
            let value = self.object_call("139", selector, "0", "0", site)?;
            if matches!(
                kind,
                Builtin::BeginTurn | Builtin::UseLifetimes | Builtin::Savepoint
            ) {
                return Ok("0".to_owned());
            }
            if kind == Builtin::Undo {
                // true when a cycle was put back, nil when there was
                // none left, which is what the reference's undo reports.
                let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
                return self.temp(format_args!("zext i32 {tag} to i128"));
            }
            let value = self.temp(format_args!("trunc i64 {value} to i32"))?;
            return self.pack(&value);
        }
        if matches!(
            kind,
            Builtin::RelationSet
                | Builtin::RelationUnset
                | Builtin::RelationGet
                | Builtin::RelationAll
                | Builtin::RelationContains
                | Builtin::RelationOutermost
                | Builtin::RelationDescendants
                | Builtin::RelationAncestors
        ) {
            let descriptor = self.integer(&arg(0), site, false)?;
            let mut descriptor = self.temp(format_args!("zext i32 {descriptor} to i64"))?;
            // a labelled relation's last argument names which table of
            // the family the row belongs to. It rides in the descriptor, since
            // the operation boundary has no fourth slot.
            // The label is an extra argument beyond what the operation takes,
            // which is how the emitter knows the relation carries one.
            let columns = match kind {
                Builtin::RelationSet | Builtin::RelationUnset | Builtin::RelationContains => 3,
                _ => 2,
            };
            let mut last = arguments.len();
            if arguments.len() > columns {
                let label = arguments.len() - 1;
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(label)))?;
                let bad = self.temp(format_args!("icmp ne i32 {tag}, 10"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(label)))?;
                let value = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                let bad = self.temp(format_args!("icmp ugt i64 {value}, 4095"))?;
                self.guard(&bad, 1, site)?;
                let shifted = self.temp(format_args!("shl i64 {value}, 16"))?;
                descriptor = self.temp(format_args!("or i64 {descriptor}, {shifted}"))?;
                last -= 1;
            }
            let mut handles = Vec::new();
            for index in 1..last {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let bad = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                handles
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(site))?;
                handles.push(self.temp(format_args!("trunc i128 {bits} to i64"))?);
            }
            let second = handles.get(1).cloned().unwrap_or_else(|| "0".to_owned());
            let op = match kind {
                Builtin::RelationSet => "131",
                Builtin::RelationUnset => "132",
                Builtin::RelationGet => "133",
                Builtin::RelationAll => "134",
                Builtin::RelationContains => "135",
                Builtin::RelationOutermost => "136",
                Builtin::RelationDescendants => "137",
                _ => "138",
            };
            let payload = self.object_call(op, &descriptor, &handles[0], &second, site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let bits = self.temp(format_args!("zext i64 {payload} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, {tag}"));
        }
        if kind == Builtin::OwnText {
            // the accumulator's slot owns a copy of the text.
            let (tag, payload) = self.tagged(&arg(0))?;
            let handle = self.object_call("127", &tag, &payload, "0", site)?;
            return self.pack_text(&handle);
        }
        if kind == Builtin::ReplaceOwner {
            // the new text takes over the slot's owner entry.
            let old = self.text_handle(&arg(0), site)?;
            let new = self.text_handle(&arg(1), site)?;
            let handle = self.object_call("128", &old, &new, "0", site)?;
            return self.pack_text(&handle);
        }
        if kind == Builtin::FlushOutput {
            // hand the gathered output to the console.
            self.object_call("126", "0", "0", "0", site)?;
            return Ok("0".to_owned());
        }
        if kind == Builtin::GetTime {
            // only the millisecond tick counter is implemented; the
            // runtime refuses the calendar mode rather than inventing one.
            let mode = self.integer(&arg(0), site, false)?;
            let mode = self.temp(format_args!("zext i32 {mode} to i64"))?;
            let ticks = self.object_call("117", &mode, "0", "0", site)?;
            let ticks = self.temp(format_args!("trunc i64 {ticks} to i32"))?;
            return self.pack(&ticks);
        }
        if kind == Builtin::NewStaticRexPattern {
            // the compiled pattern object belongs to the owner's field.
            let owner = arg(0);
            let property = arg(1);
            let owner_tag = self.temp(format_args!("trunc i128 {owner} to i32"))?;
            let wrong = self.temp(format_args!("icmp ne i32 {owner_tag}, 3"))?;
            self.guard(&wrong, 1, site)?;
            let property_tag = self.temp(format_args!("trunc i128 {property} to i32"))?;
            let wrong = self.temp(format_args!("icmp ne i32 {property_tag}, 8"))?;
            self.guard(&wrong, 1, site)?;
            let owner = self.temp(format_args!("lshr i128 {owner}, 32"))?;
            let owner = self.temp(format_args!("trunc i128 {owner} to i64"))?;
            let property = self.temp(format_args!("lshr i128 {property}, 32"))?;
            let property = self.temp(format_args!("trunc i128 {property} to i64"))?;
            let text = arg(2);
            let tag = self.temp(format_args!("trunc i128 {text} to i32"))?;
            let literal = self.temp(format_args!("icmp eq i32 {tag}, 4"))?;
            let owned = self.temp(format_args!("icmp eq i32 {tag}, 7"))?;
            let valid = self.temp(format_args!("or i1 {literal}, {owned}"))?;
            let bad = self.temp(format_args!("xor i1 {valid}, true"))?;
            self.guard(&bad, 1, site)?;
            let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
            let bits = self.temp(format_args!("lshr i128 {text}, 32"))?;
            let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let session = self.session_value()?;
            let result = self.temp(format_args!(
                "call i64 @zeb_objects_regex_call(i32 6, i64 {session}, i64 {tag}, i64 {payload}, i64 {owner}, i64 {property}, i64 0)"
            ))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            self.guard(&bad, 7, site)?;
            let bits = self.temp(format_args!("zext i64 {result} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 3"));
        }
        if matches!(kind, Builtin::NewStaticVector | Builtin::NewStaticLookup) {
            let owner = arg(0);
            let property = arg(1);
            let owner_tag = self.temp(format_args!("trunc i128 {owner} to i32"))?;
            let wrong = self.temp(format_args!("icmp ne i32 {owner_tag}, 3"))?;
            self.guard(&wrong, 1, site)?;
            let property_tag = self.temp(format_args!("trunc i128 {property} to i32"))?;
            let wrong = self.temp(format_args!("icmp ne i32 {property_tag}, 8"))?;
            self.guard(&wrong, 1, site)?;
            let owner = self.temp(format_args!("lshr i128 {owner}, 32"))?;
            let owner = self.temp(format_args!("trunc i128 {owner} to i64"))?;
            let property = self.temp(format_args!("lshr i128 {property}, 32"))?;
            let property = self.temp(format_args!("trunc i128 {property} to i64"))?;
            let result = if kind == Builtin::NewStaticLookup {
                self.object_call("95", &owner, &property, "0", site)?
            } else {
                let capacity = self.integer(&arg(2), site, false)?;
                let capacity = self.temp(format_args!("zext i32 {capacity} to i64"))?;
                self.object_call("92", &owner, &property, &capacity, site)?
            };
            let bits = self.temp(format_args!("zext i64 {result} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 3"));
        }
        if matches!(kind, Builtin::CaptureClosure | Builtin::ClosureCapture) {
            if self.word != "i128" {
                return Err(Diagnostic::new(
                    "native-profile",
                    "closures require the object profile",
                    site,
                ));
            }
            let mut payloads = Vec::new();
            for index in 0..arguments.len() {
                if kind == Builtin::ClosureCapture && index == 1 {
                    let number = self.integer(&arg(index), site, false)?;
                    payloads.push(self.temp(format_args!("zext i32 {number} to i64"))?);
                } else {
                    let expected = if kind == Builtin::ClosureCapture {
                        3
                    } else if index == 0 {
                        11
                    } else {
                        9
                    };
                    let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                    let bad = self.temp(format_args!("icmp ne i32 {tag}, {expected}"))?;
                    self.guard(&bad, 1, site)?;
                    let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                    payloads.push(self.temp(format_args!("trunc i128 {bits} to i64"))?);
                }
            }
            let value = self.object_call(
                if kind == Builtin::CaptureClosure {
                    "107"
                } else {
                    "109"
                },
                &payloads[0],
                &payloads[1],
                payloads.get(2).map_or("0", String::as_str),
                site,
            )?;
            let bits = self.temp(format_args!("zext i64 {value} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            return self.temp(format_args!("or i128 {bits}, {tag}"));
        }
        if matches!(kind, Builtin::OwnedToString | Builtin::OwnedToList) {
            let value = arg(0);
            let tag = self.temp(format_args!("trunc i128 {value} to i32"))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {value}, 32"))?;
            let buffer = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let operation = if kind == Builtin::OwnedToList {
                "100"
            } else {
                "88"
            };
            let snapshot = self.object_call(operation, &buffer, "0", "0", site)?;
            if kind == Builtin::OwnedToList {
                let bits = self.temp(format_args!("zext i64 {snapshot} to i128"))?;
                let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
                return self.temp(format_args!("or i128 {bits}, 9"));
            }
            return self.pack_text(&snapshot);
        }
        if matches!(
            kind,
            Builtin::CallQualified
                | Builtin::CallDelegated
                | Builtin::ApplyQualified
                | Builtin::ApplyMethod
        ) {
            let mut payloads = Vec::new();
            payloads
                .try_reserve(3)
                .map_err(|_| Diagnostic::resource(site))?;
            for (index, expected) in [3, 3, 8].into_iter().enumerate() {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let bad = self.temp(format_args!("icmp ne i32 {tag}, {expected}"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                payloads.push(self.temp(format_args!("trunc i128 {bits} to i64"))?);
            }
            // `delegated` may target an unrelated object; `inherited X` may not.
            if !matches!(kind, Builtin::ApplyMethod | Builtin::CallDelegated) {
                let related = self.object_call("62", &payloads[0], &payloads[1], "0", site)?;
                let bad = self.temp(format_args!("icmp eq i64 {related}, 0"))?;
                self.guard(&bad, 8, site)?;
            }
            let frame = self.object_call("63", "0", "0", "0", site)?;
            let list_value = if matches!(kind, Builtin::ApplyQualified | Builtin::ApplyMethod) {
                arg(3)
            } else {
                let list =
                    self.object_call("73", &(arguments.len() - 3).to_string(), "0", "0", site)?;
                for index in 3..arguments.len() {
                    let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                    let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
                    let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                    let value = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                    self.object_call("74", &list, &tag, &value, site)?;
                }
                self.object_call("38", &list, "0", "0", site)?;
                let bits = self.temp(format_args!("zext i64 {list} to i128"))?;
                let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
                self.temp(format_args!("or i128 {bits}, 9"))?
            };
            let tag = self.temp(format_args!("trunc i128 {list_value} to i32"))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 9"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {list_value}, 32"))?;
            let list = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let length = self.object_call("39", &list, "9", "0", site)?;
            let function = self.object_call("4", &payloads[1], &payloads[2], "0", site)?;
            let raw_kind = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let method = self.temp(format_args!("icmp eq i32 {raw_kind}, 5"))?;
            let label = self.next;
            self.next = self.next.checked_add(1).ok_or(Diagnostic::resource(site))?;
            self.line(format_args!(
                "  br i1 {method}, label %qcall{label}, label %qdata{label}\nqdata{label}:"
            ))?;
            let bad = self.temp(format_args!("icmp ne i64 {length}, 0"))?;
            self.guard(&bad, 8, site)?;
            let missing = self.temp(format_args!("icmp eq i32 {raw_kind}, 6"))?;
            let tag = self.temp(format_args!("zext i32 {raw_kind} to i128"))?;
            let bits = self.temp(format_args!("zext i64 {function} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            let data = self.temp(format_args!("or i128 {bits}, {tag}"))?;
            let data = self.temp(format_args!("select i1 {missing}, i128 0, i128 {data}"))?;
            self.object_call("65", &frame, "0", "0", site)?;
            self.line(format_args!("  br label %qdatadone{label}\nqdatadone{label}:\n  br label %qjoin{label}\nqcall{label}:"))?;
            let index = self.temp(format_args!("trunc i64 {function} to i32"))?;
            self.object_call("55", &payloads[0], "0", "0", site)?;
            let out = self.temp(format_args!(
                "call %out @zapply_method(i128 {}, i32 {index}, i128 {list_value})",
                arg(0)
            ))?;
            self.object_call("56", &payloads[0], "0", "0", site)?;
            self.object_call("65", &frame, "0", "0", site)?;
            let code = self.temp(format_args!("extractvalue %out {out}, 1"))?;
            let bad = self.temp(format_args!("icmp ne i32 {code}, 0"))?;
            self.line(format_args!(
                "  br i1 {bad}, label %qerror{label}, label %qok{label}\nqerror{label}:"
            ))?;
            self.propagate(&out, self.exception)?;
            self.line(format_args!("qok{label}:"))?;
            let value = self.temp(format_args!("extractvalue %out {out}, 0"))?;
            let owner = self.temp(format_args!("extractvalue %out {out}, 3"))?;
            let bits = self.temp(format_args!("lshr i128 {value}, 32"))?;
            let object = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            self.object_call("84", &owner, &object, "0", site)?;
            self.line(format_args!("  br label %qcalldone{label}\nqcalldone{label}:\n  br label %qjoin{label}\nqjoin{label}:"))?;
            return self.temp(format_args!(
                "phi i128 [ {data}, %qdatadone{label} ], [ {value}, %qcalldone{label} ]"
            ));
        }
        if kind == Builtin::ApplyConstructor {
            let mut payloads = Vec::new();
            payloads
                .try_reserve(3)
                .map_err(|_| Diagnostic::resource(site))?;
            for (index, expected) in [3, 8, 9].into_iter().enumerate() {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let bad = self.temp(format_args!("icmp ne i32 {tag}, {expected}"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                payloads.push(self.temp(format_args!("trunc i128 {bits} to i64"))?);
            }
            let length = self.object_call("39", &payloads[2], "9", "0", site)?;
            let function = self.object_call("4", &payloads[0], &payloads[1], "0", site)?;
            let kind = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let method = self.temp(format_args!("icmp eq i32 {kind}, 5"))?;
            let label = self.next;
            self.next = self.next.checked_add(1).ok_or(Diagnostic::resource(site))?;
            self.line(format_args!("  br i1 {method}, label %ctorapply{label}, label %ctordata{label}\nctordata{label}:"))?;
            let bad = self.temp(format_args!("icmp ne i64 {length}, 0"))?;
            self.guard(&bad, 8, site)?;
            self.line(format_args!(
                "  br label %ctorend{label}\nctorapply{label}:"
            ))?;
            let function = self.temp(format_args!("trunc i64 {function} to i32"))?;
            self.object_call("55", &payloads[0], "0", "0", site)?;
            let out = self.temp(format_args!(
                "call %out @zapply_method(i128 {}, i32 {function}, i128 {})",
                arg(0),
                arg(2)
            ))?;
            self.object_call("56", &payloads[0], "0", "0", site)?;
            let code = self.temp(format_args!("extractvalue %out {out}, 1"))?;
            let bad = self.temp(format_args!("icmp ne i32 {code}, 0"))?;
            self.line(format_args!(
                "  br i1 {bad}, label %ctorerr{label}, label %ctorend{label}\nctorerr{label}:"
            ))?;
            self.propagate(&out, self.exception)?;
            self.line(format_args!("ctorend{label}:"))?;
            return Ok("0".to_owned());
        }
        if kind == Builtin::MoveFieldOwnerTo {
            let mut payloads = Vec::new();
            payloads
                .try_reserve(4)
                .map_err(|_| Diagnostic::resource(site))?;
            for index in 0..4 {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let expected = if index % 2 == 0 { 3 } else { 8 };
                let bad = self.temp(format_args!("icmp ne i32 {tag}, {expected}"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                payloads.push(self.temp(format_args!("trunc i128 {bits} to i64"))?);
            }
            let high = self.temp(format_args!("shl i64 {}, 32", payloads[3]))?;
            let fields = self.temp(format_args!("or i64 {}, {high}", payloads[1]))?;
            let moved = self.object_call("102", &payloads[0], &payloads[2], &fields, site)?;
            return self.temp(format_args!("zext i64 {moved} to i128"));
        }
        if kind == Builtin::MoveReturnOwner {
            let value = arg(0);
            let tag = self.temp(format_args!("trunc i128 {value} to i32"))?;
            let object = self.temp(format_args!("icmp eq i32 {tag}, 3"))?;
            let text = self.temp(format_args!("icmp eq i32 {tag}, 7"))?;
            let valid = self.temp(format_args!("or i1 {object}, {text}"))?;
            let list = self.temp(format_args!("icmp eq i32 {tag}, 9"))?;
            let valid = self.temp(format_args!("or i1 {valid}, {list}"))?;
            let bad = self.temp(format_args!("xor i1 {valid}, true"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {value}, 32"))?;
            let object = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let frame = self.frame.clone().ok_or_else(|| {
                Diagnostic::new(
                    "native-profile",
                    "owning returns require object runtime",
                    site,
                )
            })?;
            self.object_call("83", &object, &frame, "0", site)?;
            return Ok(value);
        }
        if matches!(
            kind,
            Builtin::BeginPublishedConstruction
                | Builtin::FinishLocalConstruction
                | Builtin::FinishException
                | Builtin::FinishPublishedConstruction
                | Builtin::MoveLocalToCollection
                | Builtin::MoveLocalToField
                | Builtin::ReserveConstruction
                | Builtin::BeginConstruction
                | Builtin::FinishConstruction
        ) {
            let mut payloads = Vec::new();
            for index in 0..arguments.len() {
                let expected = if index == 2 { 8 } else { 3 };
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let bad = if matches!(
                    kind,
                    Builtin::MoveLocalToField | Builtin::MoveLocalToCollection
                ) && index == 0
                {
                    let object = self.temp(format_args!("icmp eq i32 {tag}, 3"))?;
                    let text = self.temp(format_args!("icmp eq i32 {tag}, 7"))?;
                    let list = self.temp(format_args!("icmp eq i32 {tag}, 9"))?;
                    let valid = self.temp(format_args!("or i1 {object}, {text}"))?;
                    let valid = self.temp(format_args!("or i1 {valid}, {list}"))?;
                    self.temp(format_args!("xor i1 {valid}, true"))?
                } else {
                    self.temp(format_args!("icmp ne i32 {tag}, {expected}"))?
                };
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                payloads
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(site))?;
                payloads.push(self.temp(format_args!("trunc i128 {bits} to i64"))?);
            }
            let value = if kind == Builtin::BeginConstruction {
                self.object_call("43", &payloads[0], "0", "0", site)?
            } else if kind == Builtin::BeginPublishedConstruction {
                self.object_call("46", &payloads[0], "0", "0", site)?
            } else if kind == Builtin::FinishLocalConstruction {
                self.object_call("82", &payloads[0], "0", "0", site)?
            } else if kind == Builtin::FinishException {
                self.object_call("76", &payloads[0], "0", "0", site)?;
                return Ok(arg(0));
            } else if kind == Builtin::FinishPublishedConstruction {
                self.object_call("48", &payloads[0], "0", "0", site)?
            } else if kind == Builtin::MoveLocalToCollection {
                self.object_call("99", &payloads[0], &payloads[1], "0", site)?;
                return Ok("0".to_owned());
            } else if kind == Builtin::MoveLocalToField {
                self.object_call("96", &payloads[0], &payloads[1], &payloads[2], site)?;
                return Ok("0".to_owned());
            } else if kind == Builtin::ReserveConstruction {
                self.object_call("47", &payloads[0], &payloads[1], &payloads[2], site)?;
                return Ok("0".to_owned());
            } else {
                self.object_call("44", &payloads[0], &payloads[1], &payloads[2], site)?
            };
            let value = self.temp(format_args!("zext i64 {value} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {value}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, 3"));
        }
        if matches!(
            kind,
            Builtin::NewOwnedCollection
                | Builtin::ReserveInCollection
                | Builtin::RemoveOwned
                | Builtin::MoveOwnedTo
                | Builtin::OwnedLength
                | Builtin::OwnedAt
        ) {
            let mut payloads = ["0".to_owned(), "0".to_owned(), "0".to_owned()];
            for (index, payload) in payloads.iter_mut().enumerate().take(arguments.len()) {
                let expected = if kind == Builtin::OwnedAt && index == 1 {
                    2
                } else {
                    3
                };
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let bad = self.temp(format_args!("icmp ne i32 {tag}, {expected}"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                *payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            }
            let op = match kind {
                Builtin::NewOwnedCollection => "50",
                Builtin::ReserveInCollection => "51",
                Builtin::RemoveOwned => "52",
                Builtin::MoveOwnedTo => "101",
                Builtin::OwnedLength => "53",
                _ => "54",
            };
            let value = self.object_call(op, &payloads[0], &payloads[1], &payloads[2], site)?;
            if kind == Builtin::ReserveInCollection {
                return Ok("0".to_owned());
            }
            let wide = self.temp(format_args!("zext i64 {value} to i128"))?;
            if matches!(kind, Builtin::RemoveOwned | Builtin::MoveOwnedTo) {
                return Ok(wide);
            }
            let bits = self.temp(format_args!("shl i128 {wide}, 32"))?;
            return self.temp(format_args!(
                "or i128 {bits}, {}",
                if kind == Builtin::OwnedLength { 2 } else { 3 }
            ));
        }
        if matches!(
            kind,
            Builtin::BeginIteration
                | Builtin::AdvanceIteration
                | Builtin::IterationValue
                | Builtin::EndIteration
        ) {
            if self.word != "i128" {
                return Err(Diagnostic::new(
                    "native-profile",
                    "iteration requires the object profile",
                    site,
                ));
            }
            if kind == Builtin::BeginIteration {
                let tag = self.temp(format_args!("trunc i128 {} to i64", arg(0)))?;
                let tag = self.temp(format_args!("and i64 {tag}, 4294967295"))?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
                let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                self.object_call("57", &tag, &payload, "0", site)?;
                return Ok("0".to_owned());
            }
            let op = match kind {
                Builtin::AdvanceIteration => "58",
                Builtin::IterationValue => "59",
                _ => "60",
            };
            let payload = self.object_call(op, "0", "0", "0", site)?;
            let wide = self.temp(format_args!("zext i64 {payload} to i128"))?;
            if kind != Builtin::IterationValue {
                return Ok(wide);
            }
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {wide}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, {tag}"));
        }
        if matches!(kind, Builtin::FirstObj | Builtin::NextObj) {
            if self.word != "i128" {
                return Err(Diagnostic::new(
                    "native-profile",
                    "enumeration requires the object profile",
                    site,
                ));
            }
            let next = usize::from(kind == Builtin::NextObj);
            let mut handles = vec!["0".to_owned()];
            for index in 0..=next {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let bad = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                handles.push(self.temp(format_args!("trunc i128 {bits} to i64"))?);
            }
            // the flag may be left off, and then means instances —
            // the reference's default, and the case an author wants. It was
            // required here, so the familiar spelling did not compile and the
            // spelling that did compile did not run.
            let flags = if arguments.len() > next + 1 {
                let flags = self.integer(&arg(next + 1), site, false)?;
                self.temp(format_args!("zext i32 {flags} to i64"))?
            } else {
                "1".to_owned()
            };
            let result =
                self.object_call("106", &handles[next], &handles[next + 1], &flags, site)?;
            let wide = self.temp(format_args!("zext i64 {result} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {wide}, 32"))?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            return self.temp(format_args!("or i128 {bits}, {tag}"));
        }
        if matches!(kind, Builtin::OfKind | Builtin::PropDefined) {
            if self.word != "i128" {
                return Err(Diagnostic::new(
                    "native-profile",
                    "kind queries require the object profile",
                    site,
                ));
            }
            let mut handles = [String::new(), String::new()];
            for (index, handle) in handles.iter_mut().enumerate() {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let expected = if kind == Builtin::PropDefined && index == 1 {
                    8
                } else {
                    3
                };
                let bad = self.temp(format_args!("icmp ne i32 {tag}, {expected}"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                *handle = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            }
            // propDefined takes the reference's optional PropDefXxx mode.
            let mode = if kind == Builtin::PropDefined && arguments.len() == 3 {
                let value = self.integer(&arg(2), site, false)?;
                self.temp(format_args!("zext i32 {value} to i64"))?
            } else {
                "0".to_owned()
            };
            let result = self.object_call(
                if kind == Builtin::PropDefined {
                    "105"
                } else {
                    "62"
                },
                &handles[0],
                &handles[1],
                &mode,
                site,
            )?;
            if kind == Builtin::PropDefined {
                // PropDefGetClass answers with the defining object; the other
                // modes answer true or nil.
                let raw = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
                let tag = self.temp(format_args!("zext i32 {raw} to i128"))?;
                let bits = self.temp(format_args!("zext i64 {result} to i128"))?;
                let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
                let object = self.temp(format_args!("or i128 {bits}, {tag}"))?;
                let logical = self.temp(format_args!("zext i64 {result} to i128"))?;
                let is_object = self.temp(format_args!("icmp eq i32 {raw}, 3"))?;
                return self.temp(format_args!(
                    "select i1 {is_object}, i128 {object}, i128 {logical}"
                ));
            }
            return self.temp(format_args!("zext i64 {result} to i128"));
        }
        if kind == Builtin::VectorInsert {
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let vector = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let index = self.integer(&arg(1), site, false)?;
            let index = self.temp(format_args!("zext i32 {index} to i64"))?;
            let frame = self.object_call("63", "0", "0", "0", site)?;
            let values =
                self.object_call("73", &(arguments.len() - 2).to_string(), "0", "0", site)?;
            for index in 2..arguments.len() {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                let value = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                self.object_call("74", &values, &tag, &value, site)?;
            }
            self.object_call("38", &values, "0", "0", site)?;
            self.object_call("104", &vector, &index, &values, site)?;
            self.object_call("65", &frame, "0", "0", site)?;
            return Ok(arg(0));
        }
        if matches!(kind, Builtin::VectorRemoveRange | Builtin::VectorRemoveAt) {
            let mut payloads = ["0".to_owned(), "0".to_owned(), "0".to_owned()];
            for (index, payload) in payloads.iter_mut().enumerate().take(arguments.len()) {
                let tag = self.temp(format_args!("trunc i128 {} to i32", arg(index)))?;
                let expected = if index == 0 { 3 } else { 2 };
                let bad = self.temp(format_args!("icmp ne i32 {tag}, {expected}"))?;
                self.guard(&bad, 1, site)?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(index)))?;
                *payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            }
            if kind == Builtin::VectorRemoveAt {
                payloads[2] = payloads[1].clone();
            }
            self.object_call("71", &payloads[0], &payloads[1], &payloads[2], site)?;
            return Ok(arg(0));
        }
        if matches!(
            kind,
            Builtin::VectorSortBegin | Builtin::VectorSortNext | Builtin::VectorSortElement
        ) {
            // runtime ops 112-114 drive the resumable quicksort.
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let vector = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(1)))?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(1)))?;
            let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            if kind == Builtin::VectorSortBegin {
                // Any non-nil order argument sorts descending, as in the reference.
                let descending = self.temp(format_args!("icmp ne i32 {tag}, 0"))?;
                let descending = self.temp(format_args!("zext i1 {descending} to i64"))?;
                self.object_call("112", &vector, &descending, "0", site)?;
                return Ok("0".to_owned());
            }
            if kind == Builtin::VectorSortNext {
                let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
                let more = self.object_call("113", &vector, &tag, &payload, site)?;
                return self.temp(format_args!("zext i64 {more} to i128"));
            }
            let value = self.object_call("114", &vector, &payload, "0", site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let value = self.temp(format_args!("zext i64 {value} to i128"))?;
            let value = self.temp(format_args!("shl i128 {value}, 32"))?;
            return self.temp(format_args!("or i128 {value}, {tag}"));
        }
        if matches!(kind, Builtin::VectorAppend | Builtin::VectorIndexOf) {
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let bad = if kind == Builtin::VectorIndexOf {
                // asking where something is in a sequence is the same
                // question of a list and of a vector, and TADS answers it of
                // both. Appending is not: a list does not change, so that keeps
                // the vector guard.
                let not_vector = self.temp(format_args!("icmp ne i32 {tag}, 3"))?;
                let not_list = self.temp(format_args!("icmp ne i32 {tag}, 9"))?;
                self.temp(format_args!("and i1 {not_vector}, {not_list}"))?
            } else {
                self.temp(format_args!("icmp ne i32 {tag}, 3"))?
            };
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let handle = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(1)))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i64"))?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(1)))?;
            let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
            if kind == Builtin::VectorIndexOf {
                let index = self.object_call("103", &handle, &tag, &payload, site)?;
                let absent = self.temp(format_args!("icmp eq i64 {index}, 0"))?;
                let bits = self.temp(format_args!("zext i64 {index} to i128"))?;
                let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
                let value = self.temp(format_args!("or i128 {bits}, 2"))?;
                return self.temp(format_args!("select i1 {absent}, i128 0, i128 {value}"));
            }
            self.object_call("69", &handle, &tag, &payload, site)?;
            return Ok(arg(0));
        }
        if kind == Builtin::NewLocalVector {
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let bad = self.temp(format_args!("icmp ne i32 {tag}, 2"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let capacity = self.temp(format_args!("trunc i128 {bits} to i32"))?;
            let capacity = self.temp(format_args!("zext i32 {capacity} to i64"))?;
            let object = self.object_call("66", &capacity, "0", "0", site)?;
            let object = self.temp(format_args!("zext i64 {object} to i128"))?;
            let object = self.temp(format_args!("shl i128 {object}, 32"))?;
            return self.temp(format_args!("or i128 {object}, 3"));
        }
        if matches!(
            kind,
            Builtin::PendingValue
                | Builtin::PendingCode
                | Builtin::PendingSite
                | Builtin::PendingOwner
        ) {
            let packet = self.temp(format_args!("load %out, ptr %pending_out"))?;
            let (field, ty) = match kind {
                Builtin::PendingValue => (0, "i128"),
                Builtin::PendingCode => (1, "i32"),
                Builtin::PendingSite => (2, "i64"),
                _ => (3, "i64"),
            };
            let value = self.temp(format_args!("extractvalue %out {packet}, {field}"))?;
            return if field == 0 {
                Ok(value)
            } else {
                self.temp(format_args!("zext {ty} {value} to i128"))
            };
        }
        if matches!(kind, Builtin::ClaimException | Builtin::DetachException) {
            let token = self.temp(format_args!("trunc i128 {} to i64", arg(0)))?;
            let result = self.object_call(
                if kind == Builtin::ClaimException {
                    "79"
                } else {
                    "80"
                },
                &token,
                "0",
                "0",
                site,
            )?;
            return self.temp(format_args!("zext i64 {result} to i128"));
        }
        if kind == Builtin::RestorePending {
            let code = self.temp(format_args!("trunc i128 {} to i32", arg(1)))?;
            let site = self.temp(format_args!("trunc i128 {} to i64", arg(2)))?;
            let packet = self.temp(format_args!(
                "insertvalue %out zeroinitializer, i128 {}, 0",
                arg(0)
            ))?;
            let packet = self.temp(format_args!("insertvalue %out {packet}, i32 {code}, 1"))?;
            let packet = self.temp(format_args!("insertvalue %out {packet}, i64 {site}, 2"))?;
            let owner = self.temp(format_args!("trunc i128 {} to i64", arg(3)))?;
            let packet = self.temp(format_args!("insertvalue %out {packet}, i64 {owner}, 3"))?;
            self.line(format_args!("  store %out {packet}, ptr %pending_out"))?;
            return Ok("0".to_owned());
        }
        if matches!(
            kind,
            Builtin::BeginScope | Builtin::EndScope | Builtin::UnwindScope
        ) {
            if self.word != "i128" {
                return Err(Diagnostic::new(
                    "native-profile",
                    "exception scopes require the object profile",
                    site,
                ));
            }
            if kind == Builtin::BeginScope {
                let mark = self.object_call("63", "0", "0", "0", site)?;
                return self.temp(format_args!("zext i64 {mark} to i128"));
            }
            let mark = self.temp(format_args!("trunc i128 {} to i64", arg(0)))?;
            self.object_call(
                if kind == Builtin::UnwindScope {
                    "65"
                } else {
                    "64"
                },
                &mark,
                "0",
                "0",
                site,
            )?;
            return Ok("0".to_owned());
        }
        if matches!(kind, Builtin::Invoke | Builtin::Apply) {
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            let named = self.temp(format_args!("icmp eq i32 {tag}, 11"))?;
            let captured = self.temp(format_args!("icmp eq i32 {tag}, 3"))?;
            let valid = if kind == Builtin::Invoke {
                self.temp(format_args!("or i1 {named}, {captured}"))?
            } else {
                named.clone()
            };
            let bad = self.temp(format_args!("xor i1 {valid}, true"))?;
            self.guard(&bad, 1, site)?;
            let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
            let index = self.temp(format_args!("trunc i128 {bits} to i32"))?;
            let mut args = format!("i32 {index}");
            for value in &arguments[1..] {
                write!(args, ", i128 %v{}", value.0).map_err(|_| Diagnostic::resource(site))?;
            }
            let out = if kind == Builtin::Apply {
                self.temp(format_args!("call %out @zapply({args})"))?
            } else {
                let label = self.next;
                self.next += 1;
                self.line(format_args!(
                    "  br i1 {captured}, label %closure{label}, label %named{label}\nnamed{label}:"
                ))?;
                let plain = self.temp(format_args!(
                    "call %out @zinvoke{}({args})",
                    arguments.len() - 1
                ))?;
                self.line(format_args!("  br label %nameddone{label}\nnameddone{label}:\n  br label %calljoin{label}\nclosure{label}:"))?;
                let handle = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                let function = self.object_call("108", &handle, "0", "0", site)?;
                let function = self.temp(format_args!("trunc i64 {function} to i32"))?;
                self.object_call("55", &handle, "0", "0", site)?;
                let mut args = format!("i32 {function}, i128 {}", arg(0));
                for value in &arguments[1..] {
                    write!(args, ", i128 %v{}", value.0).map_err(|_| Diagnostic::resource(site))?;
                }
                let result = self.temp(format_args!(
                    "call %out @zinvoke{}({args})",
                    arguments.len()
                ))?;
                self.object_call("56", &handle, "0", "0", site)?;
                self.line(format_args!("  br label %closuredone{label}\nclosuredone{label}:\n  br label %calljoin{label}\ncalljoin{label}:"))?;
                self.temp(format_args!(
                    "phi %out [ {plain}, %nameddone{label} ], [ {result}, %closuredone{label} ]"
                ))?
            };
            let code = self.temp(format_args!("extractvalue %out {out}, 1"))?;
            let bad = self.temp(format_args!("icmp ne i32 {code}, 0"))?;
            let label = self.next;
            self.next += 1;
            self.line(format_args!(
                "  br i1 {bad}, label %invokeerr{label}, label %invokeok{label}\ninvokeerr{label}:"
            ))?;
            self.propagate(&out, self.exception)?;
            self.line(format_args!("invokeok{label}:"))?;
            return self.temp(format_args!("extractvalue %out {out}, 0"));
        }
        if kind == Builtin::DataType {
            let tag = self.temp(format_args!("trunc i128 {} to i32", arg(0)))?;
            // Only value kinds admitted by this compiler slice can reach here.
            // Classification does not dereference an object or owned-text borrow.
            let mut result = "0".to_owned();
            for (internal, source) in [
                (0, 1),
                (1, 2),
                (2, 7),
                (3, 5),
                (4, 8),
                (7, 8),
                (8, 6),
                (9, 10),
                (10, 15),
                (11, 12),
            ] {
                let matches = self.temp(format_args!("icmp eq i32 {tag}, {internal}"))?;
                result = self.temp(format_args!(
                    "select i1 {matches}, i32 {source}, i32 {result}"
                ))?;
            }
            let invalid = self.temp(format_args!("icmp eq i32 {result}, 0"))?;
            self.guard(&invalid, 1, site)?;
            return self.pack(&result);
        }
        if kind == Builtin::Add {
            let left = arg(0);
            let right = arg(1);
            let tag = self.temp(format_args!("trunc i128 {left} to i32"))?;
            let numeric = self.temp(format_args!("icmp eq i32 {tag}, 2"))?;
            let label = self.next;
            self.next = self.next.checked_add(1).ok_or(Diagnostic::resource(site))?;
            self.line(format_args!(
                "  br i1 {numeric}, label %addnum{label}, label %addtext{label}\naddnum{label}:"
            ))?;
            let number = self.binary(Binary::Add, &left, &right, site, (false, false), false)?;
            self.line(format_args!("  br label %addnumend{label}\naddnumend{label}:\n  br label %addjoin{label}\naddtext{label}:"))?;
            // a list joins with a list, anything else joins as text
            let list = self.temp(format_args!("icmp eq i32 {tag}, 9"))?;
            let inner = self.next;
            self.next = self.next.checked_add(1).ok_or(Diagnostic::resource(site))?;
            self.line(format_args!(
                "  br i1 {list}, label %addlist{inner}, label %addstr{inner}\naddlist{inner}:"
            ))?;
            let lbits = self.temp(format_args!("lshr i128 {left}, 32"))?;
            let lhandle = self.temp(format_args!("trunc i128 {lbits} to i64"))?;
            let rtag = self.temp(format_args!("trunc i128 {right} to i32"))?;
            let bad = self.temp(format_args!("icmp ne i32 {rtag}, 9"))?;
            self.guard(&bad, 1, site)?;
            let rbits = self.temp(format_args!("lshr i128 {right}, 32"))?;
            let rhandle = self.temp(format_args!("trunc i128 {rbits} to i64"))?;
            let joined = self.object_call("116", &lhandle, &rhandle, "0", site)?;
            let joined = self.temp(format_args!("zext i64 {joined} to i128"))?;
            let joined = self.temp(format_args!("shl i128 {joined}, 32"))?;
            let joined = self.temp(format_args!("or i128 {joined}, 9"))?;
            self.line(format_args!("  br label %addlistend{inner}\naddlistend{inner}:\n  br label %addjoin2{inner}\naddstr{inner}:"))?;
            let left = self.text_handle(&left, site)?;
            let right = self.text_handle(&right, site)?;
            let handle = self.object_call("22", &left, &right, "0", site)?;
            let text = self.pack_text(&handle)?;
            self.line(format_args!("  br label %addstrend{inner}\naddstrend{inner}:\n  br label %addjoin2{inner}\naddjoin2{inner}:"))?;
            let text = self.temp(format_args!(
                "phi i128 [ {joined}, %addlistend{inner} ], [ {text}, %addstrend{inner} ]"
            ))?;
            self.line(format_args!("  br label %addtextend{label}\naddtextend{label}:\n  br label %addjoin{label}\naddjoin{label}:"))?;
            return self.temp(format_args!(
                "phi i128 [ {number}, %addnumend{label} ], [ {text}, %addtextend{label} ]"
            ));
        }
        // systype.h String methods. These come before the numeric
        // parameter below, whose type guard does not apply to them.
        if matches!(kind, Builtin::StartsWith | Builtin::EndsWith) {
            let subject = self.text_handle(&arg(0), site)?;
            let other = self.text_handle(&arg(1), site)?;
            let op = if kind == Builtin::StartsWith {
                "119"
            } else {
                "120"
            };
            self.object_call(op, &subject, &other, "0", site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            return Ok(tag);
        }
        if kind == Builtin::FindText {
            let subject = self.text_handle(&arg(0), site)?;
            let other = self.text_handle(&arg(1), site)?;
            let start = if arguments.len() == 3 {
                let value = self.integer(&arg(2), site, false)?;
                self.temp(format_args!("zext i32 {value} to i64"))?
            } else {
                "0".to_owned()
            };
            let found = self.object_call("121", &subject, &other, &start, site)?;
            let tag = self.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = self.temp(format_args!("zext i32 {tag} to i128"))?;
            let bits = self.temp(format_args!("zext i64 {found} to i128"))?;
            let bits = self.temp(format_args!("shl i128 {bits}, 32"))?;
            return self.temp(format_args!("or i128 {bits}, {tag}"));
        }
        if matches!(kind, Builtin::ToLower | Builtin::ToUpper | Builtin::Htmlify) {
            let subject = self.text_handle(&arg(0), site)?;
            let flags = if arguments.len() == 2 {
                let value = self.integer(&arg(1), site, false)?;
                self.temp(format_args!("zext i32 {value} to i64"))?
            } else {
                "0".to_owned()
            };
            let op = match kind {
                Builtin::ToLower => "122",
                Builtin::ToUpper => "123",
                _ => "124",
            };
            let handle = self.object_call(op, &subject, &flags, "0", site)?;
            return self.pack_text(&handle);
        }
        if kind == Builtin::FindReplace {
            // The pattern and replacement may each be text or a list of
            // text, so the runtime inspects their tags.
            let subject = self.text_handle(&arg(0), site)?;
            let (pattern_tag, pattern) = self.tagged(&arg(1))?;
            let (replacement_tag, replacement) = self.tagged(&arg(2))?;
            let flags = if arguments.len() >= 4 {
                let value = self.integer(&arg(3), site, false)?;
                self.temp(format_args!("zext i32 {value} to i64"))?
            } else {
                "0".to_owned()
            };
            let shifted = self.temp(format_args!("shl i64 {pattern_tag}, 32"))?;
            let tags = self.temp(format_args!("or i64 {shifted}, {replacement_tag}"))?;
            let session = self.session_value()?;
            let handle = self.temp(format_args!(
                "call i64 @zeb_objects_regex_call(i32 8, i64 {session}, i64 {tags}, i64 {pattern}, i64 {subject}, i64 {replacement}, i64 {flags})"
            ))?;
            let status = self.temp(format_args!("call i32 @zeb_objects_status()"))?;
            let bad = self.temp(format_args!("icmp ne i32 {status}, 0"))?;
            self.guard(&bad, 7, site)?;
            return self.pack_text(&handle);
        }
        let parameter = if arguments.len() > 1 {
            let value = self.integer(&arg(1), site, false)?;
            self.temp(format_args!("zext i32 {value} to i64"))?
        } else {
            "10".to_owned()
        };
        match kind {
            // whatever the value is, as text. Op 34 formats by tag,
            // the same formatting an emitting interpolation writes out.
            Builtin::ValueToText => {
                let kind = self.temp(format_args!("trunc i128 {} to i64", arg(0)))?;
                let kind = self.temp(format_args!("and i64 {kind}, 4294967295"))?;
                let bits = self.temp(format_args!("lshr i128 {}, 32", arg(0)))?;
                let payload = self.temp(format_args!("trunc i128 {bits} to i64"))?;
                let handle = self.object_call("145", &kind, &payload, "0", site)?;
                self.pack_text(&handle)
            }
            Builtin::ToString => {
                let value = self.integer(&arg(0), site, false)?;
                let value = self.temp(format_args!("zext i32 {value} to i64"))?;
                let decimal = self.temp(format_args!("icmp eq i64 {parameter}, 10"))?;
                let signed = self.temp(format_args!("zext i1 {decimal} to i64"))?;
                let handle = self.object_call("25", &value, &parameter, &signed, site)?;
                self.pack_text(&handle)
            }
            Builtin::ToInteger => {
                let text = self.text_handle(&arg(0), site)?;
                let result = self.object_call("24", &text, &parameter, "0", site)?;
                let bits = self.temp(format_args!("trunc i64 {result} to i32"))?;
                self.pack(&bits)
            }
            Builtin::Substr => {
                let length = if arguments.len() == 3 {
                    let value = self.integer(&arg(2), site, false)?;
                    self.temp(format_args!("zext i32 {value} to i64"))?
                } else {
                    "18446744073709551615".to_owned()
                };
                let text = self.text_handle(&arg(0), site)?;
                let handle = self.object_call("23", &text, &parameter, &length, site)?;
                self.pack_text(&handle)
            }
            Builtin::BeginIteration
            | Builtin::AdvanceIteration
            | Builtin::IterationValue
            | Builtin::EndIteration
            | Builtin::VectorAppend
            | Builtin::VectorIndexOf
            | Builtin::VectorRemoveAt
            | Builtin::VectorRemoveRange
            | Builtin::VectorInsert
            | Builtin::VectorSort
            | Builtin::VectorSortBegin
            | Builtin::VectorSortNext
            | Builtin::VectorSortElement
            | Builtin::NewStaticLookup
            | Builtin::NewStaticVector
            | Builtin::NewLocalVector
            | Builtin::NewLocalStringBuffer
            | Builtin::NewLocalLookup
            | Builtin::NewLocalLookupSized
            | Builtin::LookupSet
            | Builtin::IndexSet
            | Builtin::LookupDefault
            | Builtin::LookupSetDefault
            | Builtin::LookupBuckets
            | Builtin::LookupLength
            | Builtin::LookupContains
            | Builtin::NewOwnedCollection
            | Builtin::ReserveInCollection
            | Builtin::RemoveOwned
            | Builtin::MoveOwnedTo
            | Builtin::MoveFieldOwnerTo
            | Builtin::OwnedLength
            | Builtin::OwnedAt
            | Builtin::FirstObj
            | Builtin::NextObj
            | Builtin::PropDefined
            | Builtin::OfKind
            | Builtin::BeginScope
            | Builtin::EndScope
            | Builtin::UnwindScope
            | Builtin::PendingValue
            | Builtin::PendingCode
            | Builtin::ClaimException
            | Builtin::DetachException
            | Builtin::PendingOwner
            | Builtin::PendingSite
            | Builtin::RestorePending
            | Builtin::Apply
            | Builtin::Invoke
            | Builtin::BeginPublishedConstruction
            | Builtin::FinishLocalConstruction
            | Builtin::CaptureClosure
            | Builtin::ClosureCapture
            | Builtin::OwnedToString
            | Builtin::OwnedToList
            | Builtin::ApplyConstructor
            | Builtin::CallQualified
            | Builtin::CallDelegated
            | Builtin::ApplyQualified
            | Builtin::ApplyMethod
            | Builtin::MoveReturnOwner
            | Builtin::FinishException
            | Builtin::FinishPublishedConstruction
            | Builtin::MoveLocalToCollection
            | Builtin::NewLocalOwnedCollection
            | Builtin::MoveLocalToField
            | Builtin::ReserveConstruction
            | Builtin::BeginConstruction
            | Builtin::FinishConstruction
            | Builtin::Add
            | Builtin::DataType
            | Builtin::ArgumentPack
            | Builtin::ArgumentPackTail
            | Builtin::List
            | Builtin::ListIndex
            | Builtin::Length
            | Builtin::DictionaryAdd
            | Builtin::DictionaryRemove
            | Builtin::DictionaryFind
            | Builtin::DictionaryDefined
            | Builtin::GrammarParse
            | Builtin::GrammarBegin
            | Builtin::GrammarFinish
            | Builtin::IndexWhich
            | Builtin::ValWhich
            | Builtin::LastIndexWhich
            | Builtin::CountWhich
            | Builtin::ForEachItem
            | Builtin::Subset
            | Builtin::MapAll
            | Builtin::RexMatch
            | Builtin::NewStaticRexPattern
            | Builtin::RexSearch
            | Builtin::RexGroup
            | Builtin::RexReplace
            | Builtin::PreinitMode
            | Builtin::RunGc
            | Builtin::DebugTrace
            | Builtin::GlobalSymbols
            | Builtin::GetTime
            | Builtin::InputKey
            | Builtin::InputLine
            | Builtin::FlushOutput
            | Builtin::OwnText
            | Builtin::ReplaceOwner
            | Builtin::RelationSet
            | Builtin::RelationUnset
            | Builtin::RelationGet
            | Builtin::RelationAll
            | Builtin::RelationContains
            | Builtin::RelationOutermost
            | Builtin::RelationDescendants
            | Builtin::RelationAncestors
            | Builtin::UseLifetimes
            | Builtin::BeginTurn
            | Builtin::EndTurn
            | Builtin::TurnJournalLength
            | Builtin::StartsWith
            | Builtin::EndsWith
            | Builtin::FindText
            | Builtin::ToLower
            | Builtin::ToUpper
            | Builtin::Htmlify
            | Builtin::FindReplace
            | Builtin::Savepoint
            | Builtin::Despawn
            | Builtin::Event
            | Builtin::EventValue
            | Builtin::SnapshotToken
            | Builtin::EventSubject
            | Builtin::HostAction
            | Builtin::EngineQuery
            | Builtin::Undo
            | Builtin::UndoDepth
            | Builtin::VocabularyWords
            | Builtin::VocabularyNames
            | Builtin::ConstantList => unreachable!(),
        }
    }
    fn classify(&mut self, value: &str) -> Result<String, Diagnostic> {
        let symbol = self.runtime.as_ref().expect("runtime mode").clone();
        let address = self.temp(format_args!(
            "getelementptr %scalar_api, ptr @{symbol}, i32 0, i32 2"
        ))?;
        let function = self.temp(format_args!("load ptr, ptr {address}"))?;
        self.temp(format_args!("call i32 {function}(i64 {value})"))
    }
    fn integer(&mut self, value: &str, site: usize, proven: bool) -> Result<String, Diagnostic> {
        let word = self.word;
        if proven {
            self.line(format_args!(
                "  ; optimized integer-check source-byte {site}: proven integer"
            ))?;
            return self.unbox_known(value);
        }
        let tag = if self.runtime.is_some() {
            self.classify(value)?
        } else {
            self.temp(format_args!("trunc {word} {value} to i32"))?
        };
        let bad = self.temp(format_args!("icmp ne i32 {tag}, 2"))?;
        self.guard(&bad, 1, site)?;
        let bits = self.temp(format_args!("lshr {word} {value}, 32"))?;
        self.temp(format_args!("trunc {word} {bits} to i32"))
    }
    fn logical(&mut self, value: &str, site: usize, proven: bool) -> Result<(), Diagnostic> {
        let word = self.word;
        if proven {
            return self.line(format_args!(
                "  ; optimized logical-check source-byte {site}: proven nil/true"
            ));
        }
        let bad = if self.runtime.is_some() {
            let tag = self.classify(value)?;
            self.temp(format_args!("icmp ugt i32 {tag}, 1"))?
        } else {
            self.temp(format_args!("icmp ugt {word} {value}, 1"))?
        };
        self.guard(&bad, 1, site)
    }
    fn property_key(
        &mut self,
        property: crate::ir::PropertyKey,
        site: usize,
    ) -> Result<String, Diagnostic> {
        match property {
            crate::ir::PropertyKey::Named(index) => Ok(index.to_string()),
            crate::ir::PropertyKey::Computed(value) => {
                let word = self.word;
                let tag = self.temp(format_args!("trunc {word} %v{} to i32", value.0))?;
                let invalid = self.temp(format_args!("icmp ne i32 {tag}, 8"))?;
                self.guard(&invalid, 1, site)?;
                let bits = self.temp(format_args!("lshr {word} %v{}, 32", value.0))?;
                let payload = self.temp(format_args!("trunc {word} {bits} to i64"))?;
                Ok(payload)
            }
        }
    }
    fn pack(&mut self, int: &str) -> Result<String, Diagnostic> {
        let word = self.word;
        let wide = self.temp(format_args!("zext i32 {int} to {word}"))?;
        let bits = self.temp(format_args!("shl {word} {wide}, 32"))?;
        self.temp(format_args!("or {word} {bits}, 2"))
    }
    fn checked_wide(
        &mut self,
        wide: &str,
        site: usize,
        proven: bool,
    ) -> Result<String, Diagnostic> {
        let word = self.word;
        if proven {
            self.line(format_args!(
                "  ; optimized overflow-check source-byte {site}: proven integer range"
            ))?;
        } else {
            if self.optimize {
                self.line(format_args!("  ; missed overflow-check source-byte {site}: integer bounds do not prove safety"))?;
            }
            let low = self.temp(format_args!("icmp slt {word} {wide}, -2147483648"))?;
            let high = self.temp(format_args!("icmp sgt {word} {wide}, 2147483647"))?;
            let bad = self.temp(format_args!("or i1 {low}, {high}"))?;
            self.guard(&bad, 2, site)?;
        }
        let int = self.temp(format_args!("trunc {word} {wide} to i32"))?;
        self.pack(&int)
    }
    fn unary(
        &mut self,
        op: Unary,
        value: &str,
        site: usize,
        proven: bool,
        bounded: bool,
    ) -> Result<String, Diagnostic> {
        let word = self.word;
        if op == Unary::Not {
            self.logical(value, site, proven)?;
            return self.temp(format_args!("xor {word} {value}, 1"));
        }
        let int = self.integer(value, site, proven)?;
        match op {
            Unary::Positive => Ok(value.to_owned()),
            Unary::Negative => {
                let wide = self.temp(format_args!("sext i32 {int} to {word}"))?;
                let neg = self.temp(format_args!("sub {word} 0, {wide}"))?;
                self.checked_wide(&neg, site, bounded)
            }
            Unary::BitNot => {
                let inv = self.temp(format_args!("xor i32 {int}, -1"))?;
                self.pack(&inv)
            }
            Unary::Not => unreachable!(),
        }
    }
    fn binary(
        &mut self,
        op: Binary,
        left: &str,
        right: &str,
        site: usize,
        proven: (bool, bool),
        bounded: bool,
    ) -> Result<String, Diagnostic> {
        let word = self.word;
        if matches!(op, Binary::Equal | Binary::NotEqual) && word == "i128" {
            let lt = self.temp(format_args!("trunc i128 {left} to i32"))?;
            let rt = self.temp(format_args!("trunc i128 {right} to i32"))?;
            let left_list = self.temp(format_args!("icmp eq i32 {lt}, 9"))?;
            let right_list = self.temp(format_args!("icmp eq i32 {rt}, 9"))?;
            let lists = self.temp(format_args!("and i1 {left_list}, {right_list}"))?;
            let ll = self.temp(format_args!("icmp eq i32 {lt}, 4"))?;
            let rl = self.temp(format_args!("icmp eq i32 {rt}, 4"))?;
            let lo = self.temp(format_args!("icmp eq i32 {lt}, 7"))?;
            let ro = self.temp(format_args!("icmp eq i32 {rt}, 7"))?;
            let ls = self.temp(format_args!("or i1 {ll}, {lo}"))?;
            let rs = self.temp(format_args!("or i1 {rl}, {ro}"))?;
            let both = self.temp(format_args!("and i1 {ls}, {rs}"))?;
            let label = self.next;
            self.next = self.next.checked_add(1).ok_or(Diagnostic::resource(site))?;
            self.line(format_args!(
                "  br i1 {both}, label %texteq{label}, label %valueeq{label}\ntexteq{label}:"
            ))?;
            let lb = self.temp(format_args!("lshr i128 {left}, 32"))?;
            let rb = self.temp(format_args!("lshr i128 {right}, 32"))?;
            let lp = self.temp(format_args!("trunc i128 {lb} to i64"))?;
            let rp = self.temp(format_args!("trunc i128 {rb} to i64"))?;
            let lf = self.temp(format_args!("zext i1 {ll} to i64"))?;
            let rf = self.temp(format_args!("select i1 {rl}, i64 2, i64 0"))?;
            let flags = self.temp(format_args!("or i64 {lf}, {rf}"))?;
            let equal = self.object_call("32", &lp, &rp, &flags, site)?;
            let equal = self.temp(format_args!("trunc i64 {equal} to i1"))?;
            self.line(format_args!("  br label %texteqend{label}\ntexteqend{label}:\n  br label %eqjoin{label}\nvalueeq{label}:\n  br i1 {lists}, label %listeq{label}, label %identityeq{label}\nlisteq{label}:"))?;
            // Two lists compare by content in the runtime.
            let lb = self.temp(format_args!("lshr i128 {left}, 32"))?;
            let rb = self.temp(format_args!("lshr i128 {right}, 32"))?;
            let lp = self.temp(format_args!("trunc i128 {lb} to i64"))?;
            let rp = self.temp(format_args!("trunc i128 {rb} to i64"))?;
            let list_equal = self.object_call("110", &lp, &rp, "0", site)?;
            let list_equal = self.temp(format_args!("trunc i64 {list_equal} to i1"))?;
            self.line(format_args!("  br label %listeqend{label}\nlisteqend{label}:\n  br label %eqjoin{label}\nidentityeq{label}:"))?;
            let identity = self.temp(format_args!("icmp eq i128 {left}, {right}"))?;
            self.line(format_args!("  br label %eqjoin{label}\neqjoin{label}:"))?;
            let equal = self.temp(format_args!(
                "phi i1 [ {equal}, %texteqend{label} ], [ {list_equal}, %listeqend{label} ], [ {identity}, %identityeq{label} ]"
            ))?;
            let result = if op == Binary::NotEqual {
                self.temp(format_args!("xor i1 {equal}, true"))?
            } else {
                equal
            };
            return self.temp(format_args!("zext i1 {result} to i128"));
        }
        if matches!(op, Binary::Equal | Binary::NotEqual) {
            let cmp = self.temp(format_args!(
                "icmp {} {word} {left}, {right}",
                if op == Binary::Equal { "eq" } else { "ne" }
            ))?;
            return self.temp(format_args!("zext i1 {cmp} to {word}"));
        }
        let a = self.integer(left, site, proven.0)?;
        let b = self.integer(right, site, proven.1)?;
        if matches!(op, Binary::Add | Binary::Subtract | Binary::Multiply) {
            let a = self.temp(format_args!("sext i32 {a} to {word}"))?;
            let b = self.temp(format_args!("sext i32 {b} to {word}"))?;
            let wide = self.temp(format_args!(
                "{} {word} {a}, {b}",
                match op {
                    Binary::Add => "add",
                    Binary::Subtract => "sub",
                    _ => "mul",
                }
            ))?;
            return self.checked_wide(&wide, site, bounded);
        }
        let result = match op {
            Binary::Divide | Binary::Remainder => {
                let zero = self.temp(format_args!("icmp eq i32 {b}, 0"))?;
                self.guard(&zero, 3, site)?;
                let min = self.temp(format_args!("icmp eq i32 {a}, -2147483648"))?;
                let neg = self.temp(format_args!("icmp eq i32 {b}, -1"))?;
                let overflow = self.temp(format_args!("and i1 {min}, {neg}"))?;
                if op == Binary::Divide {
                    self.guard(&overflow, 2, site)?;
                    self.temp(format_args!("sdiv i32 {a}, {b}"))?
                } else {
                    let divisor =
                        self.temp(format_args!("select i1 {overflow}, i32 1, i32 {b}"))?;
                    self.temp(format_args!("srem i32 {a}, {divisor}"))?
                }
            }
            Binary::ShiftLeft | Binary::ShiftRight | Binary::ShiftUnsigned => {
                let invalid = self.temp(format_args!("icmp ugt i32 {b}, 31"))?;
                self.guard(&invalid, 4, site)?;
                self.temp(format_args!(
                    "{} i32 {a}, {b}",
                    match op {
                        Binary::ShiftLeft => "shl",
                        Binary::ShiftRight => "ashr",
                        _ => "lshr",
                    }
                ))?
            }
            Binary::And | Binary::Or | Binary::Xor => self.temp(format_args!(
                "{} i32 {a}, {b}",
                match op {
                    Binary::And => "and",
                    Binary::Or => "or",
                    _ => "xor",
                }
            ))?,
            Binary::Less | Binary::Greater | Binary::LessEqual | Binary::GreaterEqual => {
                let cmp = self.temp(format_args!(
                    "icmp {} i32 {a}, {b}",
                    match op {
                        Binary::Less => "slt",
                        Binary::Greater => "sgt",
                        Binary::LessEqual => "sle",
                        _ => "sge",
                    }
                ))?;
                return self.temp(format_args!("zext i1 {cmp} to {word}"));
            }
            _ => unreachable!(),
        };
        self.pack(&result)
    }
}

pub fn emit(ast: &Ast, target: Target) -> Result<String, Diagnostic> {
    emit_module(ast, target, None, false, None, None, false)
}

fn dispatch_emitter() -> Emitter {
    Emitter {
        optimize: false,
        specialized: false,
        word: "i128",
        runtime: None,
        out: Text(String::new()),
        next: 0,
        frame: None,
        exception: None,
        session: None,
        epilogue: None,
    }
}
fn boxed_handle(e: &mut Emitter, handle: &str, tag: u32) -> Result<String, Diagnostic> {
    let bits = e.temp(format_args!("zext i64 {handle} to i128"))?;
    let bits = e.temp(format_args!("shl i128 {bits}, 32"))?;
    e.temp(format_args!("or i128 {bits}, {tag}"))
}
fn accepts_arity(parameters: usize, rest: bool, optional: usize, arity: usize) -> bool {
    let fixed = parameters - usize::from(rest);
    arity >= fixed - optional && (rest || arity <= fixed)
}

fn dispatch_apply(ast: &Ast, method: bool) -> Result<String, Diagnostic> {
    let mut e = dispatch_emitter();
    let signature = if method {
        "@zapply_method(i128 %self, i32 %function, i128 %arguments)"
    } else {
        "@zapply(i32 %function, i128 %arguments)"
    };
    e.line(format_args!(
        "define internal %out {signature} {{\nentry:\n  %ret_out = alloca %out"
    ))?;
    let callable = |name: &str, parameters: usize| {
        if method {
            name.starts_with('$') && !name.starts_with("$callback") && parameters > 0
        } else {
            !name.starts_with('$') || name.starts_with("$callback") || name.contains('.')
        }
    };
    e.hoist_session()?;
    e.frame = Some(e.object_call("63", "0", "0", "0", 0)?);
    let label = e.next;
    e.next = e.next.checked_add(1).ok_or(Diagnostic::resource(0))?;
    e.epilogue = Some(label);
    let tag = e.temp(format_args!("trunc i128 %arguments to i32"))?;
    let bad = e.temp(format_args!("icmp ne i32 {tag}, 9"))?;
    e.guard(&bad, 1, 0)?;
    let bits = e.temp(format_args!("lshr i128 %arguments, 32"))?;
    let list = e.temp(format_args!("trunc i128 {bits} to i64"))?;
    let length = e.object_call("39", &list, "9", "0", 0)?;
    e.line(format_args!("  switch i32 %function, label %invalid ["))?;
    for (index, id) in ast.functions.iter().enumerate() {
        if let Syntax::Function {
            name, parameters, ..
        } = &ast.nodes[id.0].syntax
            && callable(name, parameters.len())
        {
            e.line(format_args!("    i32 {index}, label %f{index}"))?;
        }
    }
    e.line(format_args!("  ]\ninvalid:"))?;
    e.exit("{ i128 0, i32 8, i64 0, i64 0 }")?;
    for (index, id) in ast.functions.iter().enumerate() {
        let Syntax::Function {
            name,
            parameters,
            rest,
            optional,
            ..
        } = &ast.nodes[id.0].syntax
        else {
            continue;
        };
        if !callable(name, parameters.len()) {
            continue;
        }
        e.line(format_args!("f{index}:"))?;
        let fixed = parameters.len() - usize::from(*rest) - usize::from(method);
        let required = fixed - optional;
        let bad = e.temp(format_args!("icmp ult i64 {length}, {required}"))?;
        e.guard(&bad, 8, 0)?;
        if !rest {
            let bad = e.temp(format_args!("icmp ugt i64 {length}, {fixed}"))?;
            e.guard(&bad, 8, 0)?;
        }
        let mut args = if method {
            "i128 %self".to_owned()
        } else {
            String::new()
        };
        for i in 0..fixed {
            if i >= required {
                let supplied = e.temp(format_args!("icmp ugt i64 {length}, {i}"))?;
                e.line(format_args!(
                    "  %optional{index}_{i} = alloca i128
  store i128 0, ptr %optional{index}_{i}
  br i1 {supplied}, label %optional_read{index}_{i}, label %optional_done{index}_{i}
optional_read{index}_{i}:"
                ))?;
            }
            let payload = e.object_call("40", &list, &(i + 1).to_string(), "0", 0)?;
            let tag = e.temp(format_args!("call i32 @zeb_objects_kind()"))?;
            let tag = e.temp(format_args!("zext i32 {tag} to i128"))?;
            let bits = e.temp(format_args!("zext i64 {payload} to i128"))?;
            let bits = e.temp(format_args!("shl i128 {bits}, 32"))?;
            let mut value = e.temp(format_args!("or i128 {bits}, {tag}"))?;
            if i >= required {
                e.line(format_args!(
                    "  store i128 {value}, ptr %optional{index}_{i}
  br label %optional_done{index}_{i}
optional_done{index}_{i}:"
                ))?;
                value = e.temp(format_args!("load i128, ptr %optional{index}_{i}"))?;
            }
            write!(
                args,
                "{}i128 {value}",
                if i == 0 && !method { "" } else { ", " }
            )
            .map_err(|_| Diagnostic::resource(0))?;
        }
        if *rest {
            let tail = e.object_call("75", &list, &required.to_string(), "0", 0)?;
            let value = boxed_handle(&mut e, &tail, 9)?;
            write!(
                args,
                "{}i128 {value}",
                if fixed == 0 && !method { "" } else { ", " }
            )
            .map_err(|_| Diagnostic::resource(0))?;
        }
        let result = e.temp(format_args!("call %out @zfn{index}({args})"))?;
        e.exit(&result)?;
    }
    e.write_epilogue()?;
    e.line(format_args!("}}"))?;
    Ok(e.out.0)
}
fn dispatch_rest_wrapper(index: usize, arity: usize) -> Result<String, Diagnostic> {
    let mut e = dispatch_emitter();
    let mut args = String::new();
    for i in 0..arity {
        write!(args, "{}i128 %a{i}", if i == 0 { "" } else { ", " })
            .map_err(|_| Diagnostic::resource(0))?;
    }
    e.line(format_args!(
        "define internal %out @zvfn{index}_{arity}({args}) {{\nentry:"
    ))?;
    e.frame = Some(e.object_call("63", "0", "0", "0", 0)?);
    let list = e.object_call("73", &arity.to_string(), "0", "0", 0)?;
    for i in 0..arity {
        let tag = e.temp(format_args!("trunc i128 %a{i} to i32"))?;
        let tag = e.temp(format_args!("zext i32 {tag} to i64"))?;
        let bits = e.temp(format_args!("lshr i128 %a{i}, 32"))?;
        let payload = e.temp(format_args!("trunc i128 {bits} to i64"))?;
        e.object_call("74", &list, &tag, &payload, 0)?;
    }
    e.object_call("38", &list, "0", "0", 0)?;
    let value = boxed_handle(&mut e, &list, 9)?;
    let result = e.temp(format_args!("call %out @zapply(i32 {index}, i128 {value})"))?;
    e.exit(&result)?;
    e.line(format_args!("}}"))?;
    Ok(e.out.0)
}

/// Internal wide-value profile; initializes named objects separately on a fresh session.
pub fn emit_objects(ast: &Ast, target: Target) -> Result<String, Diagnostic> {
    let mut module = emit_module(ast, target, None, false, None, None, true)?;
    let initializer = crate::object_init::emit_declarations(ast)?;
    for line in initializer
        .lines()
        .filter(|line| !line.starts_with("declare "))
    {
        module
            .try_reserve(line.len() + 1)
            .map_err(|_| Diagnostic::resource(0))?;
        module.push_str(line);
        module.push('\n');
    }
    // A capturing callback is dispatched with its environment argument first.
    let capturing = crate::sema::capturing_callbacks(ast)?;
    let effective = |index: usize, parameters: usize| {
        parameters + usize::from(capturing.get(index).copied().unwrap_or(false))
    };
    let mut arities = vec![0];
    for node in &ast.nodes {
        if let Syntax::Call(_, arguments) = &node.syntax
            && !arities.contains(&arguments.len())
        {
            arities
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(0))?;
            arities.push(arguments.len());
        }
    }
    for index in 0..arities.len() {
        let captured_arity = arities[index]
            .checked_add(1)
            .ok_or(Diagnostic::resource(0))?;
        if !arities.contains(&captured_arity) {
            arities
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(0))?;
            arities.push(captured_arity);
        }
    }
    for arity in &arities {
        write!(
            module,
            "\ndefine internal %out @zinvoke{arity}(i32 %function"
        )
        .map_err(|_| Diagnostic::resource(0))?;
        for i in 0..*arity {
            write!(module, ", i128 %a{i}").map_err(|_| Diagnostic::resource(0))?;
        }
        module.push_str(") {\nentry:\n  switch i32 %function, label %invalid [\n");
        for (index, id) in ast.functions.iter().enumerate() {
            if let Syntax::Function {
                name,
                parameters,
                rest,
                optional,
                ..
            } = &ast.nodes[id.0].syntax
                && (!name.starts_with('$') || name.starts_with("$callback"))
                && accepts_arity(effective(index, parameters.len()), *rest, *optional, *arity)
            {
                writeln!(module, "    i32 {index}, label %f{index}")
                    .map_err(|_| Diagnostic::resource(0))?;
            }
        }
        module.push_str("  ]\ninvalid:\n  ret %out { i128 0, i32 8, i64 0, i64 0 }\n");
        for (index, id) in ast.functions.iter().enumerate() {
            if let Syntax::Function {
                name,
                parameters,
                rest,
                optional,
                ..
            } = &ast.nodes[id.0].syntax
                && (!name.starts_with('$') || name.starts_with("$callback"))
                && accepts_arity(effective(index, parameters.len()), *rest, *optional, *arity)
            {
                let callee = if *rest || *optional != 0 {
                    format!("zvfn{index}_{arity}")
                } else {
                    format!("zfn{index}")
                };
                write!(module, "f{index}:\n  %r{index} = call %out @{callee}(")
                    .map_err(|_| Diagnostic::resource(0))?;
                for i in 0..*arity {
                    if i > 0 {
                        module.push_str(", ");
                    }
                    write!(module, "i128 %a{i}").map_err(|_| Diagnostic::resource(0))?;
                }
                writeln!(module, ")\n  ret %out %r{index}").map_err(|_| Diagnostic::resource(0))?;
            }
        }
        module.push_str("}\n");
        for (index, id) in ast.functions.iter().enumerate() {
            if let Syntax::Function {
                name,
                parameters,
                rest,
                optional,
                ..
            } = &ast.nodes[id.0].syntax
                && (!name.starts_with('$') || name.starts_with("$callback"))
                && (*rest || *optional != 0)
                && accepts_arity(parameters.len(), *rest, *optional, *arity)
            {
                module.push_str(&dispatch_rest_wrapper(index, *arity)?);
            }
        }
    }
    module.push_str(&dispatch_apply(ast, false)?);
    module.push_str(&dispatch_apply(ast, true)?);

    for arity in arities {
        write!(
            module,
            "\ndefine internal %out @zdispatch{arity}(i128 %self, i32 %method"
        )
        .map_err(|_| Diagnostic::resource(0))?;
        for i in 0..arity {
            write!(module, ", i128 %a{i}").map_err(|_| Diagnostic::resource(0))?;
        }
        module.push_str(") {\nentry:\n  switch i32 %method, label %invalid [\n");
        for (index, id) in ast.functions.iter().enumerate() {
            if let Syntax::Function {
                name,
                parameters,
                rest,
                optional,
                ..
            } = &ast.nodes[id.0].syntax
                && name.starts_with('$')
                && !name.starts_with("$callback")
                && accepts_arity(parameters.len(), *rest, *optional, arity + 1)
            {
                writeln!(module, "    i32 {index}, label %m{index}")
                    .map_err(|_| Diagnostic::resource(0))?;
            }
        }
        module.push_str("  ]\ninvalid:\n  ret %out { i128 0, i32 8, i64 0, i64 0 }\n");
        for (index, id) in ast.functions.iter().enumerate() {
            if let Syntax::Function {
                name,
                parameters,
                rest,
                optional,
                ..
            } = &ast.nodes[id.0].syntax
                && name.starts_with('$')
                && !name.starts_with("$callback")
                && accepts_arity(parameters.len(), *rest, *optional, arity + 1)
            {
                let callee = if *rest || *optional != 0 {
                    format!("zvfn{index}_{}", arity + 1)
                } else {
                    format!("zfn{index}")
                };
                write!(
                    module,
                    "m{index}:\n  %r{index} = call %out @{callee}(i128 %self"
                )
                .map_err(|_| Diagnostic::resource(0))?;
                for i in 0..arity {
                    write!(module, ", i128 %a{i}").map_err(|_| Diagnostic::resource(0))?;
                }
                writeln!(module, ")\n  ret %out %r{index}").map_err(|_| Diagnostic::resource(0))?;
            }
        }
        module.push_str("}\n");
        for (index, id) in ast.functions.iter().enumerate() {
            if let Syntax::Function {
                name,
                parameters,
                rest,
                optional,
                ..
            } = &ast.nodes[id.0].syntax
                && name.starts_with('$')
                && !name.starts_with("$callback")
                && (*rest || *optional != 0)
                && accepts_arity(parameters.len(), *rest, *optional, arity + 1)
            {
                module.push_str(&dispatch_rest_wrapper(index, arity + 1)?);
            }
        }
    }
    /*
     * The grammar and literal tables, carried by the program .
     *
     * They are module data and a loop rather than one call per word: the blob
     * is tens of kilobytes, and a call site each would be thousands of lines
     * of IR for something that runs once. The boundary stays pointer-free —
     * the loop reads its own constant and passes words .
     */
    {
        let blob = crate::tables::blob(ast)?;
        let mut escaped = String::new();
        for byte in &blob {
            escaped
                .try_reserve(4)
                .map_err(|_| Diagnostic::resource(0))?;
            write!(escaped, "\\{byte:02X}").map_err(|_| Diagnostic::resource(0))?;
        }
        // Padded to a whole number of words, so the loop never reads past the
        // array; op 150 is told the true length and ignores the padding.
        let bytes = blob.len().div_ceil(8) * 8;
        for _ in blob.len()..bytes {
            escaped
                .try_reserve(4)
                .map_err(|_| Diagnostic::resource(0))?;
            escaped.push_str("\\00");
        }
        writeln!(
            module,
            "\n@zeb_tables = private unnamed_addr constant [{bytes} x i8] c\"{escaped}\", align 8"
        )
        .map_err(|_| Diagnostic::resource(0))?;
        writeln!(
            module,
            "\ndefine %out @zeb_install_tables() {{\nentry:\n  %session = call i64 @zeb_objects_session()\n  call i64 @zeb_objects_call(i32 148, i64 %session, i64 {}, i64 0, i64 0)\n  %begin_status = call i32 @zeb_objects_status()\n  %began = icmp ne i32 %begin_status, 0\n  br i1 %began, label %failed, label %loop\nloop:\n  %i = phi i64 [ 0, %entry ], [ %next, %body ]\n  %more = icmp ult i64 %i, {bytes}\n  br i1 %more, label %body, label %done\nbody:\n  %at = getelementptr [{bytes} x i8], ptr @zeb_tables, i64 0, i64 %i\n  %word = load i64, ptr %at, align 8\n  call i64 @zeb_objects_call(i32 149, i64 %session, i64 %word, i64 0, i64 0)\n  %next = add i64 %i, 8\n  br label %loop\ndone:\n  call i64 @zeb_objects_call(i32 150, i64 %session, i64 {}, i64 0, i64 0)\n  %end_status = call i32 @zeb_objects_status()\n  %ended = icmp ne i32 %end_status, 0\n  br i1 %ended, label %failed, label %ok\nfailed:\n  ret %out {{ i128 0, i32 7, i64 0, i64 0 }}\nok:\n  ret %out zeroinitializer\n}}",
            blob.len(),
            blob.len()
        )
        .map_err(|_| Diagnostic::resource(0))?;
    }
    module.push_str("\ndefine %out @zeb_run_static_initializers() {\nentry:\n");
    if let Some(index) = ast.functions.iter().position(|id| matches!(&ast.nodes[id.0].syntax, Syntax::Function { name, .. } if name == "$initializers")) {
        writeln!(module, "  %result = call %out @zfn{index}()\n  ret %out %result\n}}").map_err(|_| Diagnostic::resource(0))?;
    } else { module.push_str("  ret %out zeroinitializer\n}\n"); }
    // a distinct phase, so a failure reports as preinit rather than
    // as static initialization. The order inside it was settled while parsing.
    module.push_str("\ndefine %out @zeb_run_preinit() {\nentry:\n");
    if let Some(index) = ast.functions.iter().position(
        |id| matches!(&ast.nodes[id.0].syntax, Syntax::Function { name, .. } if name == "$preinit"),
    ) {
        writeln!(
            module,
            "  %result = call %out @zfn{index}()\n  ret %out %result\n}}"
        )
        .map_err(|_| Diagnostic::resource(0))?;
    } else {
        module.push_str("  ret %out zeroinitializer\n}\n");
    }
    Ok(module)
}

/// Source-aware optimizations; conservative emission remains available through `emit`.
pub fn emit_optimized(ast: &Ast, target: Target) -> Result<String, Diagnostic> {
    emit_module(ast, target, None, true, None, None, false)
}

/// Optimized closed-world scalar emission from explicit native entry functions.
pub fn emit_rooted(ast: &Ast, target: Target, roots: &[usize]) -> Result<String, Diagnostic> {
    emit_module(ast, target, None, true, Some(roots), None, false)
}

pub fn emit_with_runtime_rooted(
    ast: &Ast,
    target: Target,
    symbol: &str,
    roots: &[usize],
) -> Result<String, Diagnostic> {
    validate_runtime_symbol(symbol)?;
    emit_module(ast, target, Some(symbol), true, Some(roots), None, false)
}

/// Private scalar API symbol must come from a validated matching runtime artifact.
pub fn emit_with_runtime(ast: &Ast, target: Target, symbol: &str) -> Result<String, Diagnostic> {
    validate_runtime_symbol(symbol)?;
    emit_module(ast, target, Some(symbol), false, None, None, false)
}
/// Candidate reservations for the two native entry kinds, indexed by source function.
/// These are not certified frame sizes; production builders must not assume they are.
#[derive(Clone, Copy)]
pub struct StackCharge {
    pub general: u64,
    pub integer: u64,
}

/// Feasibility emission only. Each entry receives an initial i64 remaining-byte
/// argument excluding its own charge. The caller must admit the root separately.
/// No CLI builder uses this until final-object and host certification exist.
pub fn emit_stack_candidate(
    ast: &Ast,
    target: Target,
    optimize: bool,
    charges: &[StackCharge],
) -> Result<String, Diagnostic> {
    validate_stack_charges(ast, charges)?;
    emit_module(ast, target, None, optimize, None, Some(charges), false)
}

fn validate_stack_charges(ast: &Ast, charges: &[StackCharge]) -> Result<(), Diagnostic> {
    if charges.len() != ast.functions.len()
        || charges.iter().any(|c| c.general == 0 || c.integer == 0)
    {
        return Err(Diagnostic::new(
            "stack-charges",
            "every source function needs positive native entry charges",
            0,
        ));
    }
    Ok(())
}

/// Experimental guarded scalar C entry, ABI version 3. Its byte budget excludes
/// the wrapper/host reservation and includes the source root. No frame certificate
/// or physical stack guarantee follows from a caller-supplied budget/charge table.
pub fn emit_stack_entry_candidate(
    ast: &Ast,
    target: Target,
    optimize: bool,
    charges: &[StackCharge],
    root: usize,
    symbol: &str,
) -> Result<String, Diagnostic> {
    emit_stack_entry_with_runtime_candidate(ast, target, optimize, charges, root, symbol, None)
}

/// Candidate guarded entry using the matching private scalar runtime, when supplied.
/// Runtime calls are not charged here: final runtime closure and host capacity must
/// be established before any production adoption. This also shares all validation
/// and wrapper generation with the source-only candidate.
pub fn emit_stack_entry_with_runtime_candidate(
    ast: &Ast,
    target: Target,
    optimize: bool,
    charges: &[StackCharge],
    root: usize,
    symbol: &str,
    runtime: Option<&str>,
) -> Result<String, Diagnostic> {
    if let Some(runtime) = runtime {
        validate_runtime_symbol(runtime)?;
    }
    validate_stack_charges(ast, charges)?;
    if !symbol.starts_with("zeb_stack_candidate_")
        || !symbol
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        || ast
            .nodes
            .iter()
            .any(|n| n.start > u32::MAX as usize || n.end > u32::MAX as usize)
    {
        return Err(Diagnostic::new(
            "stack-entry",
            "invalid candidate symbol or source offset range",
            0,
        ));
    }
    let function = ast
        .functions
        .get(root)
        .and_then(|id| ast.nodes.get(id.0))
        .ok_or_else(|| Diagnostic::new("stack-entry", "invalid source root", 0))?;
    let crate::parser::Syntax::Function { parameters, .. } = &function.syntax else {
        return Err(Diagnostic::new(
            "stack-entry",
            "root is not a function",
            function.start,
        ));
    };
    if parameters.len() > 3 {
        return Err(Diagnostic::new(
            "stack-entry",
            "candidate entry supports zero to three integer inputs",
            function.start,
        ));
    }
    let mut module = Text(emit_module(
        ast,
        target,
        runtime,
        optimize,
        None,
        Some(charges),
        false,
    )?);
    let mut parameters_text = String::new();
    let mut packing = String::new();
    let mut arguments = String::from("i64 %remaining");
    for i in 0..parameters.len() {
        write!(parameters_text, ", i32 %arg{i}").map_err(|_| Diagnostic::resource(0))?;
        write!(packing, "  %wide{i} = zext i32 %arg{i} to i64\n  %bits{i} = shl i64 %wide{i}, 32\n  %packed{i} = or i64 %bits{i}, 2\n").map_err(|_| Diagnostic::resource(0))?;
        write!(arguments, ", i64 %packed{i}").map_err(|_| Diagnostic::resource(0))?;
    }
    let charge = charges[root].general;
    let root_failure = ((function.start as u64) << 32) | 8;
    write!(module, "\ndefine i64 @{symbol}(i32 %abi, i64 %budget{parameters_text}) noredzone \"target-cpu\"=\"{}\" {{\nentry:\n  %version = icmp eq i32 %abi, 3\n  br i1 %version, label %admit, label %mismatch\nmismatch:\n  ret i64 7\nadmit:\n  %small = icmp ult i64 %budget, {charge}\n  br i1 %small, label %exhausted, label %invoke\nexhausted:\n  ret i64 {root_failure}\ninvoke:\n  %remaining = sub i64 %budget, {charge}\n{packing}  %result = call %out @zfn{root}({arguments})\n  %value = extractvalue %out %result, 0\n  %code = extractvalue %out %result, 1\n  %site = extractvalue %out %result, 2\n  %failed = icmp ne i32 %code, 0\n  %kind = add i32 %code, 2\n  %tag = zext i32 %kind to i64\n  %offset = shl i64 %site, 32\n  %error = or i64 %offset, %tag\n  %word = select i1 %failed, i64 %error, i64 %value\n  ret i64 %word\n}}\n", target.cpu()).map_err(|_| Diagnostic::resource(0))?;
    Ok(module.0)
}

fn validate_runtime_symbol(symbol: &str) -> Result<(), Diagnostic> {
    if !symbol.starts_with("_R")
        || !symbol
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(Diagnostic::new(
            "runtime-symbol",
            "invalid mangled runtime symbol",
            0,
        ));
    }
    Ok(())
}
fn emit_module(
    ast: &Ast,
    target: Target,
    runtime: Option<&str>,
    optimize: bool,
    roots: Option<&[usize]>,
    charges: Option<&[StackCharge]>,
    wide: bool,
) -> Result<String, Diagnostic> {
    if !wide
        && (!ast.objects.is_empty()
            || ast.nodes.iter().any(|n| {
                matches!(
                    n.syntax,
                    crate::parser::Syntax::String(_)
                        | crate::parser::Syntax::Function { rest: true, .. }
                        | crate::parser::Syntax::Function {
                            returns_owned: true,
                            ..
                        }
                )
            }))
    {
        return Err(Diagnostic::new(
            "native-profile",
            "object values require the wide object profile",
            0,
        ));
    }
    let word = if wide { "i128" } else { "i64" };
    // the narrow profile has no text — a string literal, an object and
    // an owned return are all refused above — so its `+` is arithmetic whatever
    // the memory model is. Lowering it that way is what lets a scalar program
    // build under lifetimes, which is the default. The checked IR is
    // the emitted IR: both come from this one lowering.
    let checked = flow::check_with(ast, wide && ast.lifetimes)?;
    let owned_calls = crate::sema::owned_call_sites(ast)?;
    if !wide
        && checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(|i| {
                matches!(
                    i.operation,
                    Operation::FunctionValue(_)
                        | Operation::Builtin {
                            kind: crate::sema::Builtin::NewLocalVector
                                | crate::sema::Builtin::NewLocalLookupSized
                                | crate::sema::Builtin::Apply
                                | crate::sema::Builtin::ApplyMethod
                                | crate::sema::Builtin::Invoke
                                | crate::sema::Builtin::NewOwnedCollection
                                | crate::sema::Builtin::NewLocalOwnedCollection
                                | crate::sema::Builtin::MoveLocalToCollection
                                | crate::sema::Builtin::MoveLocalToField
                                | crate::sema::Builtin::ReserveInCollection
                                | crate::sema::Builtin::RemoveOwned
                                | crate::sema::Builtin::OwnedLength
                                | crate::sema::Builtin::OwnedAt,
                            ..
                        }
                )
            })
    {
        return Err(Diagnostic::new(
            "native-profile",
            "function and collection operations require the wide object profile",
            0,
        ));
    }
    let retained = roots
        .map(|r| crate::roots::retained(ast, &checked.program, r))
        .transpose()?;
    let summaries = if optimize {
        Some(effects::analyze(ast, &checked.program)?)
    } else {
        None
    };
    let profiles = if optimize {
        Some(crate::specialize::plan(
            ast,
            &checked.program,
            retained.as_deref(),
        )?)
    } else {
        None
    };
    let mut e = Emitter {
        frame: None,
        exception: None,
        session: None,
        epilogue: None,
        optimize,
        specialized: false,
        word,
        runtime: runtime.map(str::to_owned),
        out: Text(String::new()),
        next: 0,
    };
    let owner_type = if wide { ", i64" } else { "" };
    e.line(format_args!(
        "; Zebulon scalar native IR v1\ntarget triple = \"{}\"\n%out = type {{ {word}, i32, i64{owner_type} }}",
        target.triple()
    ))?;
    if wide {
        e.line(format_args!("declare i64 @zeb_objects_call(i32, i64, i64, i64, i64)\ndeclare i64 @zeb_objects_lookup_call(i32, i64, i64, i64, i64, i64)\ndeclare i64 @zeb_objects_dictionary_call(i32, i64, i64, i64, i64, i64, i64)\ndeclare i64 @zeb_objects_grammar_call(i32, i64, i64, i64, i64, i64)\ndeclare i64 @zeb_objects_regex_call(i32, i64, i64, i64, i64, i64, i64)\ndeclare i32 @zeb_objects_status()\ndeclare i32 @zeb_objects_kind()\ndeclare i64 @zeb_objects_session()"))?;
    }
    if charges.is_some() {
        e.line(format_args!(
            "; Candidate call-site budgets: charges and host stack are NOT certified"
        ))?;
    }
    if profiles.is_some() {
        e.line(format_args!("%iout = type {{ i32, i32, i64 }}"))?;
    }
    if let Some(symbol) = runtime {
        e.line(format_args!(
            "%scalar_api = type {{ i32, i32, ptr }}\n@{symbol} = external constant %scalar_api"
        ))?;
    }
    for (fi, f) in checked.program.functions.iter().enumerate() {
        if let Some(keep) = &retained {
            let source = ast.nodes[f.source.0].start;
            if !keep[fi] {
                e.line(format_args!("; optimized function-removal source-byte {source}: function {fi} has no path from native roots"))?;
                continue;
            }
            e.line(format_args!("; retained function source-byte {source}: function {fi} is a native root or direct-call dependency"))?;
        }
        if let Some(profiles) = &profiles
            && profiles[fi].facts.is_none()
        {
            e.line(format_args!(
                "; missed integer-entry source-byte {}: function {fi}: {}",
                ast.nodes[f.source.0].start, profiles[fi].reason
            ))?;
        }
        let ranges = if optimize {
            Some(ranges::analyze(f)?)
        } else {
            None
        };
        for specialized in [false, true] {
            if specialized && !profiles.as_ref().is_some_and(|p| p[fi].facts.is_some()) {
                continue;
            }
            e.specialized = specialized;
            e.frame = None;
            let facts = if specialized {
                profiles.as_ref().unwrap()[fi].facts.as_ref().unwrap()
            } else {
                &checked.facts[fi]
            };
            let outcome = e.outcome();
            let data = e.data_type();
            let prefix = if specialized { "zsp" } else { "zfn" };
            if specialized {
                e.line(format_args!(
                    "; specialized integer entry source-byte {}: function {fi}",
                    ast.nodes[f.source.0].start
                ))?;
            }
            let attributes = if runtime.is_none() && !wide {
                summaries.as_ref().and_then(|s| {
                    s[fi].effects.scalar_only().then_some(
                        if s[fi].effects.contains(Effect::Diverge) {
                            " memory(none) nounwind"
                        } else {
                            " memory(none) nounwind willreturn"
                        },
                    )
                })
            } else {
                None
            };
            if optimize {
                e.line(format_args!(
                    "; {} llvm-effects source-byte {}: function {fi}; {}",
                    if attributes.is_some() {
                        "optimized"
                    } else {
                        "missed"
                    },
                    ast.nodes[f.source.0].start,
                    attributes.unwrap_or("runtime or observable effects remain opaque")
                ))?;
            }
            if let Some(charges) = charges {
                let charge = if specialized {
                    charges[fi].integer
                } else {
                    charges[fi].general
                };
                e.line(format_args!(
                    "; candidate-stack-charge {prefix}{fi} {charge}"
                ))?;
            }
            e.next = 0;
            e.session = None;
            e.epilogue = None;
            e.out
                .write_fmt(format_args!("define internal {outcome} @{prefix}{fi}("))
                .map_err(|_| Diagnostic::resource(0))?;
            if charges.is_some() {
                e.out
                    .write_str("i64 %stack_remaining")
                    .map_err(|_| Diagnostic::resource(0))?;
            }
            let params = f.slots.iter().take_while(|s| s.parameter).count();
            for pi in 0..params {
                e.out
                    .write_fmt(format_args!(
                        "{}{data} %arg{pi}",
                        if pi == 0 && charges.is_none() {
                            ""
                        } else {
                            ", "
                        }
                    ))
                    .map_err(|_| Diagnostic::resource(0))?;
            }
            e.line(format_args!(
                ") \"target-cpu\"=\"{}\"{}{} {{\nentry:",
                target.cpu(),
                if charges.is_some() { " noredzone" } else { "" },
                attributes.unwrap_or("")
            ))?;
            if wide {
                e.line(format_args!(
                    "  %pending_out = alloca %out\n  store %out zeroinitializer, ptr %pending_out"
                ))?;
            }
            for si in 0..f.slots.len() {
                e.line(format_args!(
                    "  %s{si} = alloca {word}, align 8\n  store {word} 0, ptr %s{si}, align 8"
                ))?;
            }
            if runtime.is_some() {
                e.validate_runtime()?;
            }
            for pi in 0..params {
                let arg = if specialized {
                    e.pack(&format!("%arg{pi}"))?
                } else {
                    format!("%arg{pi}")
                };
                e.line(format_args!("  store {word} {arg}, ptr %s{pi}, align 8"))?;
            }
            if wide {
                /*
                 * The session, once, in the block that dominates every other
                 * . Before the frame call, which is its first reader.
                 */
                /*
                 * One place to release it from. Every `exit` stores the
                 * outcome here and jumps; the epilogue loads it, releases the
                 * frame once and returns. The slot is declared before the
                 * frame call, because that call's own guard splits the entry
                 * block and an `alloca` after it would be a dynamic one.
                 */
                e.line(format_args!("  %ret_out = alloca {outcome}"))?;
                e.hoist_session()?;
                e.frame = Some(e.object_call("63", "0", "0", "0", ast.nodes[f.source.0].start)?);
                /*
                 * Set last: the frame call's own failure path must return
                 * directly, because there is no frame yet to release.
                 */
                let label = e.next;
                e.next = e.next.checked_add(1).ok_or(Diagnostic::resource(0))?;
                e.epilogue = Some(label);
            }
            e.line(format_args!("  br label %b0"))?;
            // Keep all structurally reachable blocks. Flow-infeasible source paths remain
            // guarded by their original condition; stack slots start with defined bits.
            let mut reachable = vec![false; f.blocks.len()];
            let mut todo = vec![0];
            while let Some(b) = todo.pop() {
                if reachable[b] {
                    continue;
                }
                reachable[b] = true;
                if let Some(edge) = f.blocks[b].terminator.exception() {
                    todo.push(edge.target.0);
                }
                match f.blocks[b].terminator {
                    Terminator::Invoke { normal, .. } => todo.push(normal.0),
                    Terminator::Jump(t) => todo.push(t.0),
                    Terminator::Branch { yes, no, .. } => {
                        todo.push(yes.0);
                        todo.push(no.0);
                    }
                    _ => {}
                }
            }
            let mut local_values = Vec::new();
            if optimize {
                local_values
                    .try_reserve_exact(f.slots.len())
                    .map_err(|_| Diagnostic::resource(0))?;
                local_values.resize(f.slots.len(), None::<crate::ir::Value>);
            }
            for (bi, block) in f.blocks.iter().enumerate() {
                if !reachable[bi] {
                    continue;
                }
                // No predecessor/loop facts: only dominating values from this block.
                local_values.fill(None);
                e.exception = if let Terminator::Invoke { exception, .. } = block.terminator {
                    Some(exception)
                } else {
                    None
                };
                e.line(format_args!("b{bi}:"))?;
                for i in &block.instructions {
                    let site = ast.nodes[i.site.0].start;
                    e.line(format_args!("  ; source-byte {site} node {}", i.site.0))?;
                    let result = match &i.operation {
                        Operation::FunctionValue(value) => {
                            Some(((u64::from(*value) << 32) | 11).to_string())
                        }
                        Operation::Enumerator(value) => {
                            Some(((u64::from(*value) << 32) | 10).to_string())
                        }
                        Operation::PropertyValue(property) => {
                            Some(((u64::from(*property) << 32) | 8).to_string())
                        }
                        Operation::Constant(s) => Some(match s {
                            Scalar::Nil => "0".to_owned(),
                            Scalar::True => "1".to_owned(),
                            Scalar::Integer(v) => (((u64::from(*v as u32)) << 32) | 2).to_string(),
                        }),
                        Operation::Load(s) => {
                            if let Some(value) = local_values.get(s.0).copied().flatten() {
                                e.line(format_args!("  ; optimized local-read source-byte {site}: slot {} has dominating value", s.0))?;
                                Some(format!("%v{}", value.0))
                            } else {
                                if optimize {
                                    e.line(format_args!("  ; missed local-read source-byte {site}: slot {} has no block-local value", s.0))?;
                                    local_values[s.0] = i.result;
                                }
                                Some(e.temp(format_args!("load {word}, ptr %s{}, align 8", s.0))?)
                            }
                        }
                        Operation::Store(s, v) => {
                            if optimize {
                                local_values[s.0] = Some(*v);
                            }
                            e.line(format_args!(
                                "  store {word} %v{}, ptr %s{}, align 8",
                                v.0, s.0
                            ))?;
                            None
                        }
                        Operation::EmitValue(value) => {
                            if !wide {
                                return Err(Diagnostic::new(
                                    "native-profile",
                                    "embedded output requires the object profile",
                                    site,
                                ));
                            }
                            let kind = e.temp(format_args!("trunc i128 %v{} to i64", value.0))?;
                            let kind = e.temp(format_args!("and i64 {kind}, 4294967295"))?;
                            let bits = e.temp(format_args!("lshr i128 %v{}, 32", value.0))?;
                            let payload = e.temp(format_args!("trunc i128 {bits} to i64"))?;
                            e.object_call("33", &kind, &payload, "0", site)?;
                            None
                        }
                        Operation::EmitLiteral(literal) => {
                            let session = e.session_value()?;
                            e.temp(format_args!("call i64 @zeb_objects_call(i32 14, i64 {session}, i64 {literal}, i64 0, i64 0)"))?;
                            let status = e.temp(format_args!("call i32 @zeb_objects_status()"))?;
                            let failed = e.temp(format_args!("icmp ne i32 {status}, 0"))?;
                            e.guard(&failed, 7, site)?;
                            None
                        }
                        Operation::EndStatic => {
                            let session = e.session_value()?;
                            e.temp(format_args!("call i64 @zeb_objects_call(i32 30, i64 {session}, i64 0, i64 0, i64 0)"))?;
                            let status = e.temp(format_args!("call i32 @zeb_objects_status()"))?;
                            let bad = e.temp(format_args!("icmp ne i32 {status}, 0"))?;
                            e.guard(&bad, 7, site)?;
                            None
                        }
                        Operation::BeginStatic(receiver, property) => {
                            let bits = e.temp(format_args!("lshr i128 %v{}, 32", receiver.0))?;
                            let handle = e.temp(format_args!("trunc i128 {bits} to i64"))?;
                            let session = e.session_value()?;
                            e.temp(format_args!("call i64 @zeb_objects_call(i32 20, i64 {session}, i64 {handle}, i64 {property}, i64 0)"))?;
                            let status = e.temp(format_args!("call i32 @zeb_objects_status()"))?;
                            let bad = e.temp(format_args!("icmp ne i32 {status}, 0"))?;
                            e.guard(&bad, 7, site)?;
                            None
                        }
                        Operation::SetProperty(receiver, property, value) => {
                            let property = e.property_key(*property, site)?;
                            if !wide {
                                return Err(Diagnostic::new(
                                    "native-profile",
                                    "property assignment requires the wide object profile",
                                    site,
                                ));
                            }
                            let tag = e.temp(format_args!("trunc i128 %v{} to i32", receiver.0))?;
                            let bad = e.temp(format_args!("icmp ne i32 {tag}, 3"))?;
                            e.guard(&bad, 1, site)?;
                            let bits = e.temp(format_args!("lshr i128 %v{}, 32", receiver.0))?;
                            let handle = e.temp(format_args!("trunc i128 {bits} to i64"))?;
                            let kind = e.temp(format_args!("trunc i128 %v{} to i32", value.0))?;
                            let scalar_invalid = e.temp(format_args!("icmp ugt i32 {kind}, 4"))?;
                            let is_owned = e.temp(format_args!("icmp eq i32 {kind}, 7"))?;
                            let not_owned = e.temp(format_args!("xor i1 {is_owned}, true"))?;
                            let invalid =
                                e.temp(format_args!("and i1 {scalar_invalid}, {not_owned}"))?;
                            let is_property = e.temp(format_args!("icmp eq i32 {kind}, 8"))?;
                            let not_property =
                                e.temp(format_args!("xor i1 {is_property}, true"))?;
                            let invalid =
                                e.temp(format_args!("and i1 {invalid}, {not_property}"))?;
                            let is_list = e.temp(format_args!("icmp eq i32 {kind}, 9"))?;
                            let not_list = e.temp(format_args!("xor i1 {is_list}, true"))?;
                            let invalid = e.temp(format_args!("and i1 {invalid}, {not_list}"))?;
                            let is_enum = e.temp(format_args!("icmp eq i32 {kind}, 10"))?;
                            let not_enum = e.temp(format_args!("xor i1 {is_enum}, true"))?;
                            let invalid = e.temp(format_args!("and i1 {invalid}, {not_enum}"))?;
                            let is_function = e.temp(format_args!("icmp eq i32 {kind}, 11"))?;
                            let not_function =
                                e.temp(format_args!("xor i1 {is_function}, true"))?;
                            let invalid =
                                e.temp(format_args!("and i1 {invalid}, {not_function}"))?;
                            e.guard(&invalid, 1, site)?;
                            let base_op = e.temp(format_args!("add i32 {kind}, 5"))?;
                            let is_text = e.temp(format_args!("icmp eq i32 {kind}, 4"))?;
                            let op =
                                e.temp(format_args!("select i1 {is_text}, i32 13, i32 {base_op}"))?;
                            let op =
                                e.temp(format_args!("select i1 {is_owned}, i32 29, i32 {op}"))?;
                            let op =
                                e.temp(format_args!("select i1 {is_property}, i32 35, i32 {op}"))?;
                            let op =
                                e.temp(format_args!("select i1 {is_list}, i32 41, i32 {op}"))?;
                            let op =
                                e.temp(format_args!("select i1 {is_enum}, i32 42, i32 {op}"))?;
                            let op =
                                e.temp(format_args!("select i1 {is_function}, i32 49, i32 {op}"))?;
                            let bits = e.temp(format_args!("lshr i128 %v{}, 32", value.0))?;
                            let payload = e.temp(format_args!("trunc i128 {bits} to i64"))?;
                            let session = e.session_value()?;
                            e.temp(format_args!("call i64 @zeb_objects_call(i32 {op}, i64 {session}, i64 {handle}, i64 {property}, i64 {payload})"))?;
                            let status = e.temp(format_args!("call i32 @zeb_objects_status()"))?;
                            let bad = e.temp(format_args!("icmp ne i32 {status}, 0"))?;
                            e.guard(&bad, 7, site)?;
                            None
                        }
                        Operation::GetProperty(receiver, property)
                        | Operation::CallMethod {
                            receiver, property, ..
                        } => {
                            if !wide {
                                return Err(Diagnostic::new(
                                    "native-profile",
                                    "methods require wide object values",
                                    site,
                                ));
                            }
                            local_values.fill(None);
                            let property = e.property_key(*property, site)?;
                            let tag = e.temp(format_args!("trunc i128 %v{} to i32", receiver.0))?;
                            let bad = e.temp(format_args!("icmp ne i32 {tag}, 3"))?;
                            e.guard(&bad, 1, site)?;
                            let bits = e.temp(format_args!("lshr i128 %v{}, 32", receiver.0))?;
                            let handle = e.temp(format_args!("trunc i128 {bits} to i64"))?;
                            let session = e.session_value()?;
                            let (operation, definer) = if let Operation::CallMethod {
                                after: Some(object),
                                ..
                            } = &i.operation
                            {
                                let definer = e.temp(format_args!("call i64 @zeb_objects_call(i32 11, i64 {session}, i64 {object}, i64 0, i64 0)"))?;
                                let status =
                                    e.temp(format_args!("call i32 @zeb_objects_status()"))?;
                                let bad = e.temp(format_args!("icmp ne i32 {status}, 0"))?;
                                e.guard(&bad, 7, site)?;
                                (17, definer)
                            } else {
                                (4, "0".to_owned())
                            };
                            let payload = e.temp(format_args!("call i64 @zeb_objects_call(i32 {operation}, i64 {session}, i64 {handle}, i64 {property}, i64 {definer})"))?;
                            let status = e.temp(format_args!("call i32 @zeb_objects_status()"))?;
                            let bad = e.temp(format_args!("icmp ne i32 {status}, 0"))?;
                            e.guard(&bad, 7, site)?;
                            let raw_kind = e.temp(format_args!("call i32 @zeb_objects_kind()"))?;
                            let missing = e.temp(format_args!("icmp eq i32 {raw_kind}, 6"))?;
                            let kind =
                                e.temp(format_args!("select i1 {missing}, i32 0, i32 {raw_kind}"))?;
                            let method = e.temp(format_args!("icmp eq i32 {kind}, 5"))?;
                            let label = e.next;
                            e.next += 1;
                            e.line(format_args!("  br i1 {method}, label %method{label}, label %data{label}\nmethod{label}:"))?;
                            let index = e.temp(format_args!("trunc i64 {payload} to i32"))?;
                            let mut args = format!("i128 %v{}, i32 {index}", receiver.0);
                            let arity =
                                if let Operation::CallMethod { arguments, .. } = &i.operation {
                                    for value in arguments {
                                        write!(args, ", i128 %v{}", value.0)
                                            .map_err(|_| Diagnostic::resource(site))?;
                                    }
                                    arguments.len()
                                } else {
                                    0
                                };
                            e.object_call("55", &handle, "0", "0", site)?;
                            let out =
                                e.temp(format_args!("call %out @zdispatch{arity}({args})"))?;
                            e.object_call("56", &handle, "0", "0", site)?;
                            let code = e.temp(format_args!("extractvalue %out {out}, 1"))?;
                            let bad = e.temp(format_args!("icmp ne i32 {code}, 0"))?;
                            e.line(format_args!("  br i1 {bad}, label %methoderr{label}, label %methodok{label}\nmethoderr{label}:"))?;
                            e.propagate(&out, e.exception)?;
                            e.line(format_args!("methodok{label}:"))?;
                            let value = e.temp(format_args!("extractvalue %out {out}, 0"))?;
                            let token = e.temp(format_args!("extractvalue %out {out}, 3"))?;
                            let bits = e.temp(format_args!("lshr i128 {value}, 32"))?;
                            let object = e.temp(format_args!("trunc i128 {bits} to i64"))?;
                            e.object_call(
                                "84",
                                &token,
                                &object,
                                if owned_calls[i.site.0] { "1" } else { "0" },
                                site,
                            )?;
                            e.line(format_args!(
                                "  br label %methodclaimed{label}\nmethodclaimed{label}:"
                            ))?;
                            e.line(format_args!(
                                "  br label %propertyjoin{label}\ndata{label}:"
                            ))?;
                            if owned_calls[i.site.0] {
                                e.exit(&format!("{{ i128 0, i32 7, i64 {site}, i64 0 }}"))?;
                                e.line(format_args!("propertyjoin{label}:"))?;
                                Some(value)
                            } else if arity != 0 && operation == 17 {
                                e.line(format_args!("  br i1 {missing}, label %missing{label}, label %arityerr{label}\narityerr{label}:"))?;
                                e.exit(&format!("{{ i128 0, i32 8, i64 {site}, i64 0 }}"))?;
                                e.line(format_args!("missing{label}:\n  br label %propertyjoin{label}\npropertyjoin{label}:"))?;
                                Some(e.temp(format_args!(
                                    "phi i128 [ {value}, %methodclaimed{label} ], [ 0, %missing{label} ]"
                                ))?)
                            } else if arity != 0 {
                                e.exit(&format!("{{ i128 0, i32 8, i64 {site}, i64 0 }}"))?;
                                e.line(format_args!("propertyjoin{label}:"))?;
                                Some(value)
                            } else {
                                let logical = e.temp(format_args!("icmp ule i32 {kind}, 1"))?;
                                let tag = e.temp(format_args!("zext i32 {kind} to i128"))?;
                                let payload = e.temp(format_args!("zext i64 {payload} to i128"))?;
                                let bits = e.temp(format_args!("shl i128 {payload}, 32"))?;
                                let packed = e.temp(format_args!("or i128 {bits}, {tag}"))?;
                                let data = e.temp(format_args!(
                                    "select i1 {logical}, i128 {tag}, i128 {packed}"
                                ))?;
                                e.line(format_args!(
                                    "  br label %propertyjoin{label}\npropertyjoin{label}:"
                                ))?;
                                Some(e.temp(format_args!("phi i128 [ {value}, %methodclaimed{label} ], [ {data}, %data{label} ]"))?)
                            }
                        }
                        Operation::StringLiteral(index) | Operation::NamedObject(index) => {
                            let (operation, tag) =
                                if matches!(i.operation, Operation::StringLiteral(_)) {
                                    (12, 4)
                                } else {
                                    (11, 3)
                                };
                            let session = e.session_value()?;
                            let handle = e.temp(format_args!("call i64 @zeb_objects_call(i32 {operation}, i64 {session}, i64 {index}, i64 0, i64 0)"))?;
                            let status = e.temp(format_args!("call i32 @zeb_objects_status()"))?;
                            let bad = e.temp(format_args!("icmp ne i32 {status}, 0"))?;
                            e.guard(&bad, 7, site)?;
                            let handle = e.temp(format_args!("zext i64 {handle} to i128"))?;
                            let bits = e.temp(format_args!("shl i128 {handle}, 32"))?;
                            Some(e.temp(format_args!("or i128 {bits}, {tag}"))?)
                        }
                        Operation::Reset(s) => {
                            if optimize {
                                local_values[s.0] = None;
                            }
                            e.line(format_args!("  store {word} 0, ptr %s{}, align 8", s.0))?;
                            None
                        }
                        Operation::Unary(op, v) => Some(e.unary(
                            *op,
                            &format!("%v{}", v.0),
                            site,
                            optimize
                                && if *op == Unary::Not {
                                    matches!(facts.value_types[v.0], 2 | 4 | 6)
                                } else {
                                    facts.value_types[v.0] == 1
                                },
                            ranges.as_ref().is_some_and(|r| r[v.0].negation_safe()),
                        )?),
                        Operation::Builtin { kind, arguments } => {
                            Some(e.string_builtin(*kind, arguments, site)?)
                        }
                        Operation::Binary(op, a, b) => Some(
                            e.binary(
                                *op,
                                &format!("%v{}", a.0),
                                &format!("%v{}", b.0),
                                site,
                                (
                                    optimize && facts.value_types[a.0] == 1,
                                    optimize && facts.value_types[b.0] == 1,
                                ),
                                ranges
                                    .as_ref()
                                    .is_some_and(|r| r[a.0].arithmetic_safe(*op, r[b.0])),
                            )?,
                        ),
                        Operation::LogicalGuard(v) => {
                            e.logical(
                                &format!("%v{}", v.0),
                                site,
                                optimize && matches!(facts.value_types[v.0], 2 | 4 | 6),
                            )?;
                            None
                        }
                        Operation::Call {
                            function,
                            arguments,
                        } => {
                            // Never carry a frame-read proof across a source call.
                            local_values.fill(None);
                            let use_special = profiles
                                .as_ref()
                                .is_some_and(|p| p[*function].facts.is_some())
                                && arguments.iter().all(|v| facts.value_types[v.0] == 1);
                            let called_out = if use_special { "%iout" } else { "%out" };
                            let called_prefix = if use_special { "zsp" } else { "zfn" };
                            if optimize {
                                e.line(format_args!(
                                    "  ; {} integer-call source-byte {site}: callee {function}",
                                    if use_special { "optimized" } else { "missed" }
                                ))?;
                            }
                            let Syntax::Function {
                                rest,
                                parameters,
                                optional,
                                ..
                            } = &ast.nodes[ast.functions[*function].0].syntax
                            else {
                                unreachable!()
                            };
                            let fixed = if *rest {
                                parameters.len() - 1
                            } else {
                                arguments.len()
                            };
                            let mut rest_scope = None;
                            let mut rest_value = None;
                            if *rest {
                                let mark = e.object_call("63", "0", "0", "0", site)?;
                                let rest_start = fixed - optional;
                                let list = e.object_call(
                                    "73",
                                    &(arguments.len() - rest_start).to_string(),
                                    "0",
                                    "0",
                                    site,
                                )?;
                                for value in &arguments[rest_start..] {
                                    let tag =
                                        e.temp(format_args!("trunc i128 %v{} to i32", value.0))?;
                                    let tag = e.temp(format_args!("zext i32 {tag} to i64"))?;
                                    let bits =
                                        e.temp(format_args!("lshr i128 %v{}, 32", value.0))?;
                                    let payload =
                                        e.temp(format_args!("trunc i128 {bits} to i64"))?;
                                    e.object_call("74", &list, &tag, &payload, site)?;
                                }
                                e.object_call("38", &list, "0", "0", site)?;
                                let bits = e.temp(format_args!("zext i64 {list} to i128"))?;
                                let bits = e.temp(format_args!("shl i128 {bits}, 32"))?;
                                rest_value = Some(e.temp(format_args!("or i128 {bits}, 9"))?);
                                rest_scope = Some(mark);
                            }
                            let mut args = String::new();
                            for ai in 0..fixed {
                                let value = if let Some(a) = arguments.get(ai) {
                                    if use_special {
                                        e.unbox_known(&format!("%v{}", a.0))?
                                    } else {
                                        format!("%v{}", a.0)
                                    }
                                } else {
                                    "0".to_owned()
                                };
                                let data = if use_special { "i32" } else { word };
                                write!(args, "{}{data} {value}", if ai == 0 { "" } else { ", " })
                                    .map_err(|_| Diagnostic::resource(site))?;
                            }
                            if let Some(value) = rest_value {
                                write!(args, "{}i128 {value}", if fixed == 0 { "" } else { ", " })
                                    .map_err(|_| Diagnostic::resource(site))?;
                            }
                            if let Some(charges) = charges {
                                let charge = if use_special {
                                    charges[*function].integer
                                } else {
                                    charges[*function].general
                                };
                                let exhausted = e.temp(format_args!(
                                    "icmp ult i64 %stack_remaining, {charge}"
                                ))?;
                                e.guard(&exhausted, 6, site)?;
                                let remaining =
                                    e.temp(format_args!("sub i64 %stack_remaining, {charge}"))?;
                                args = format!(
                                    "i64 {remaining}{}{args}",
                                    if args.is_empty() { "" } else { ", " }
                                );
                            }
                            let out = e.temp(format_args!(
                                "call {called_out} @{called_prefix}{function}({args})"
                            ))?;
                            if let Some(mark) = rest_scope {
                                e.object_call("65", &mark, "0", "0", site)?;
                            }
                            if runtime.is_none()
                                && charges.is_none()
                                && summaries
                                    .as_ref()
                                    .is_some_and(|s| !s[*function].effects.contains(Effect::Throw))
                            {
                                e.line(format_args!("  ; optimized call-error-check source-byte {site}: callee {function} cannot throw; call retained"))?;
                            } else {
                                if optimize {
                                    e.line(format_args!("  ; missed call-error-check source-byte {site}: callee may throw, runtime ABI or stack admission can fail"))?;
                                }
                                let code =
                                    e.temp(format_args!("extractvalue {called_out} {out}, 1"))?;
                                let bad = e.temp(format_args!("icmp ne i32 {code}, 0"))?;
                                let label = e.next;
                                e.next += 1;
                                e.line(format_args!("  br i1 {bad}, label %callerr{label}, label %callok{label}\ncallerr{label}:"))?;
                                let failure = if outcome == called_out {
                                    out.clone()
                                } else {
                                    let offset =
                                        e.temp(format_args!("extractvalue {called_out} {out}, 2"))?;
                                    let result = e.temp(format_args!(
                                        "insertvalue {outcome} zeroinitializer, i32 {code}, 1"
                                    ))?;
                                    e.temp(format_args!(
                                        "insertvalue {outcome} {result}, i64 {offset}, 2"
                                    ))?
                                };
                                e.propagate(&failure, e.exception)?;
                                e.line(format_args!("callok{label}:"))?;
                            }
                            let value =
                                e.temp(format_args!("extractvalue {called_out} {out}, 0"))?;
                            if wide && !use_special {
                                let token = e.temp(format_args!("extractvalue %out {out}, 3"))?;
                                let bits = e.temp(format_args!("lshr i128 {value}, 32"))?;
                                let object = e.temp(format_args!("trunc i128 {bits} to i64"))?;
                                e.object_call(
                                    "84",
                                    &token,
                                    &object,
                                    if owned_calls[i.site.0] { "1" } else { "0" },
                                    site,
                                )?;
                            }
                            Some(if use_special { e.pack(&value)? } else { value })
                        }
                    };
                    if let (Some(value), Some(result)) = (i.result, result) {
                        e.line(format_args!("  %v{} = add {word} 0, {result}", value.0))?;
                    }
                }
                match block.terminator {
                    Terminator::Invoke { normal, .. } => {
                        e.line(format_args!("  br label %b{}", normal.0))?
                    }
                    Terminator::Resume(edge) => {
                        let pending = e.temp(format_args!("load %out, ptr %pending_out"))?;
                        e.propagate(&pending, edge)?;
                    }
                    Terminator::Jump(t) => e.line(format_args!("  br label %b{}", t.0))?,
                    Terminator::Branch {
                        condition,
                        mode,
                        yes,
                        no,
                    } => {
                        let cmp = e.temp(format_args!(
                            "icmp eq {word} %v{}, {}",
                            condition.0,
                            if mode == BranchMode::Logical { 1 } else { 0 }
                        ))?;
                        e.line(format_args!(
                            "  br i1 {cmp}, label %b{}, label %b{}",
                            yes.0, no.0
                        ))?;
                    }
                    Terminator::Throw(v, throw_site, edge) => {
                        if !wide || specialized {
                            return Err(Diagnostic::new(
                                "native-profile",
                                "source exceptions require the object profile",
                                ast.nodes[block.site.0].start,
                            ));
                        }
                        let site = ast.nodes[throw_site.0].start;
                        let tag = e.temp(format_args!("trunc i128 %v{} to i32", v.0))?;
                        let bad = e.temp(format_args!("icmp ne i32 {tag}, 3"))?;
                        e.guard(&bad, 1, site)?;
                        let bits = e.temp(format_args!("lshr i128 %v{}, 32", v.0))?;
                        let handle = e.temp(format_args!("trunc i128 {bits} to i64"))?;
                        e.object_call("61", &handle, "0", "0", site)?;
                        let owner = e.object_call("81", &handle, "0", "0", site)?;
                        let out = e.temp(format_args!(
                            "insertvalue %out zeroinitializer, i128 %v{}, 0",
                            v.0
                        ))?;
                        let out = e.temp(format_args!("insertvalue %out {out}, i32 9, 1"))?;
                        let out = e.temp(format_args!("insertvalue %out {out}, i64 {site}, 2"))?;
                        let out = e.temp(format_args!("insertvalue %out {out}, i64 {owner}, 3"))?;
                        e.propagate(&out, edge)?;
                    }
                    Terminator::Return(v) => {
                        let value = if specialized {
                            e.unbox_known(&format!("%v{}", v.0))?
                        } else {
                            format!("%v{}", v.0)
                        };
                        let out = e.temp(format_args!(
                            "insertvalue {outcome} zeroinitializer, {data} {value}, 0"
                        ))?;
                        let out = if matches!(
                            ast.nodes[f.source.0].syntax,
                            Syntax::Function {
                                returns_owned: true,
                                ..
                            }
                        ) {
                            let bits = e.temp(format_args!("lshr i128 {value}, 32"))?;
                            let owner = e.temp(format_args!("trunc i128 {bits} to i64"))?;
                            e.temp(format_args!("insertvalue %out {out}, i64 {owner}, 3"))?
                        } else {
                            out
                        };
                        e.exit(&out)?;
                    }
                    Terminator::Fallthrough => e.line(format_args!("  unreachable"))?,
                }
            }
            e.write_epilogue()?;
            e.line(format_args!("}}"))?;
        }
    }
    Ok(e.out.0)
}
