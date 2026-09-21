//! Host-native object-program bundles using the common LLVM emitter.
use std::{fs, path::Path};
use zeb_frontend::{
    llvm::{self, Target},
    parser::{Ast, Syntax},
    source::Source,
};
use zebc::manifest;
use zebc::toolchain::Toolchain;

const RUNTIME: &str = include_str!("../../zeb-runtime/src/lib.rs");
const GRAMMAR: &str = include_str!("../../zeb-runtime/src/grammar.rs");
const OBJECTS: &str = include_str!("../../zeb-runtime/src/objects.rs");
const SAVE: &str = include_str!("../../zeb-runtime/src/save.rs");
const PERSISTENCE: &str = include_str!("../../zeb-runtime/src/objects/persistence.rs");
const HISTORY: &str = include_str!("../../zeb-runtime/src/objects/history.rs");
const OUTPUT: &str = include_str!("../../zeb-runtime/src/output.rs");
const SORT: &str = include_str!("../../zeb-runtime/src/sort.rs");
const REGEX: &str = include_str!("../../zeb-runtime/src/regex.rs");
const RELATIONS: &str = include_str!("../../zeb-runtime/src/relations.rs");
const NATIVE: &str = include_str!("../../zeb-runtime/src/native_objects.rs");
const SESSION: &str = include_str!("../../zeb-runtime/src/session.rs");
const TABLES: &str = include_str!("../../zeb-runtime/src/tables.rs");

/// One ABI version shared by the entry check, symbol, header and manifest.
pub const ABI: u32 = 12;
pub const PROFILE: &str = "object-shared-v12";

/// Runtime identity excludes the program's grammar and literal tables.
fn runtime_identity() -> String {
    zebc::runtime_cache::identity(&[
        RUNTIME,
        OBJECTS,
        HISTORY,
        SAVE,
        PERSISTENCE,
        NATIVE,
        OUTPUT,
        SORT,
        REGEX,
        RELATIONS,
        SESSION,
        GRAMMAR,
        TABLES,
    ])
}

fn write(dir: &Path, name: &str, data: impl AsRef<[u8]>) -> Result<(), String> {
    fs::write(dir.join(name), data).map_err(|e| e.to_string())
}
fn symbol(names: &str, readable: &str, method: &str, target: Target) -> Result<String, String> {
    let suffix = format!("::native_objects::{method}");
    let matches = names
        .lines()
        .zip(readable.lines())
        .filter_map(|(raw, demangled)| {
            demangled
                .ends_with(&suffix)
                .then(|| raw.split_whitespace().last().map(|s| target.ir_symbol(s)))
                .flatten()
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!("expected one runtime {method} symbol"));
    }
    Ok(matches[0].to_owned())
}

/// What the bundle's entry runs. Under ownership that is `main` once; under
/// lifetimes it is the command cycle itself, which the host drives.
pub enum Program {
    Main(usize),
    Cycle {
        startup: Option<usize>,
        /// Called with the tokens when the host submits a line. A game that
        /// never parses declares no turn and takes actions only ; a
        /// bundle needs one entry point or the other, not both.
        turn: Option<usize>,
        /// Called with the failure code when a turn dies part-way, after the
        /// cycle's changes have been put back. Without one the
        /// runtime prints a plain line.
        recover: Option<usize>,
        /// Called when the host submits an action rather than a line, with the
        /// verb and the entities it names.
        act: Option<usize>,
        /// The word-token enumerator, which the runtime tokenizer needs.
        token: u32,
    },
}

fn entry(symbol: &str, program: &Program, cpu: &str, abi: u32) -> String {
    // the memory model is a build option, so the entry selects it
    // before anything allocates. Under ownership the runtime keeps its default.
    let select_model = match program {
        Program::Main(_) => "",
        Program::Cycle { .. } => {
            "  call i64 @zeb_objects_call(i32 139, i64 %session, i64 3, i64 0, i64 0)\n"
        }
    };
    let (body, phi) = match program {
        Program::Main(function) => (
            format!(
                "run_main:\n  %main_outcome = call %out @zfn{function}()\n  br label %finish\n"
            ),
            "[ %static_outcome, %invoke ], [ %preinit_outcome, %preinit ], [ %main_outcome, %run_main ]".to_owned(),
        ),
        Program::Cycle {
            startup,
            turn,
            recover,
            act,
            token,
        } => {
            // The cycle belongs to the host; the game's only entry points are
            // startup and one turn. An input request inside a turn suspends the
            // worker thread, and the host never blocks on the game.
            // A failed turn is reported, not fatal: the cycle's changes are
            // already back, so the next command runs against the world as it
            // stood before.
            // Two paths can fail — a turn and an action — so the fragment is
            // built per path; LLVM names are local to the function, not the block.
            let report = |tag: &str, code: &str| match recover {
                Some(recover) => format!(
                    "  %failure{tag} = zext i32 {code} to i64\n  %failure_wide{tag} = zext i64 %failure{tag} to i128\n  %failure_bits{tag} = shl i128 %failure_wide{tag}, 32\n  %failure_value{tag} = or i128 %failure_bits{tag}, 2\n  %recovered{tag} = call %out @zfn{recover}(i128 %failure_value{tag})\n"
                ),
                None => format!(
                    "  %reported{tag} = call i64 @zeb_objects_call(i32 139, i64 %session, i64 7, i64 0, i64 0)\n"
                ),
            };
            let act_report = report("_act", "%act_code");
            // an action intent reaches `act(verb, subjects)`. Without
            // one the game understands lines only, and an action ends the run.
            let act_block = match act {
                Some(act) => format!(
                    "run_act:\n  %subjects = call i64 @zeb_objects_call(i32 142, i64 %session, i64 0, i64 0, i64 0)\n  %verb_wide = zext i64 %tokens to i128\n  %verb_bits = shl i128 %verb_wide, 32\n  %verb_value = or i128 %verb_bits, 2\n  %subjects_wide = zext i64 %subjects to i128\n  %subjects_bits = shl i128 %subjects_wide, 32\n  %subjects_value = or i128 %subjects_bits, 9\n  %act_outcome = call %out @zfn{act}(i128 %verb_value, i128 %subjects_value)\n  %act_code = extractvalue %out %act_outcome, 1\n  %act_bad = icmp ne i32 %act_code, 0\n  br i1 %act_bad, label %act_failed, label %end_turn\nact_failed:\n  %act_reverted = call i64 @zeb_objects_call(i32 139, i64 %session, i64 4, i64 0, i64 0)\n{act_report}  br label %end_turn\n"
                ),
                None => "run_act:\n  br label %quiet\n".to_owned(),
            };
            // A game may take lines, actions, or both. One with no `turn` is a
            // game that never parses — a CRPG front end chooses the verb — and
            // a line arriving at it ends the run, exactly as an action arriving
            // at a parser-only game does.
            let turn_block = match turn {
                Some(turn) => {
                    let turn_report = report("_turn", "%turn_code");
                    format!(
                        "run_turn:\n  %tokens_wide = zext i64 %tokens to i128\n  %tokens_bits = shl i128 %tokens_wide, 32\n  %tokens_value = or i128 %tokens_bits, 9\n  %turn_outcome = call %out @zfn{turn}(i128 %tokens_value)\n  %turn_code = extractvalue %out %turn_outcome, 1\n  %turn_failed = icmp ne i32 %turn_code, 0\n  br i1 %turn_failed, label %failed_turn, label %end_turn\nfailed_turn:\n  %reverted = call i64 @zeb_objects_call(i32 139, i64 %session, i64 4, i64 0, i64 0)\n{turn_report}  br label %end_turn\n"
                    )
                }
                None => "run_turn:\n  br label %quiet\n".to_owned(),
            };
            let (start_block, mut phi) = match startup {
                Some(startup) => (
                    format!(
                        "run_startup:\n  %startup_outcome = call %out @zfn{startup}()\n  %startup_code = extractvalue %out %startup_outcome, 1\n  %startup_failed = icmp ne i32 %startup_code, 0\n  br i1 %startup_failed, label %finish, label %cycle\n"
                    ),
                    "[ %static_outcome, %invoke ], [ %preinit_outcome, %preinit ], [ %startup_outcome, %run_startup ]".to_owned(),
                ),
                None => (
                    String::new(),
                    "[ %static_outcome, %invoke ], [ %preinit_outcome, %preinit ]".to_owned(),
                ),
            };
            phi.push_str(", [ zeroinitializer, %quiet ]");
            (
                format!(
                    "{start_block}cycle:\n  %began = call i64 @zeb_objects_call(i32 139, i64 %session, i64 0, i64 0, i64 0)\n  %tokens = call i64 @zeb_objects_call(i32 140, i64 %session, i64 {token}, i64 0, i64 0)\n  %intent_status = call i32 @zeb_objects_status()\n  %intent_bad = icmp ne i32 %intent_status, 0\n  br i1 %intent_bad, label %intent_error, label %intent_kind\nintent_kind:\n  %intent_tag = call i32 @zeb_objects_kind()\n  %more = icmp eq i32 %intent_tag, 9\n  br i1 %more, label %run_turn, label %intent_action\nintent_action:\n  %is_action = icmp eq i32 %intent_tag, 2\n  br i1 %is_action, label %run_act, label %quiet\n{act_block}{turn_block}end_turn:\n  %released = call i64 @zeb_objects_call(i32 139, i64 %session, i64 1, i64 0, i64 0)\n  br label %cycle\nquiet:\n  br label %finish\nintent_error:\n  %intent_wide = zext i32 %intent_status to i64\n  %intent_bits = shl i64 %intent_wide, 8\n  %intent_result = or i64 %intent_bits, 8\n  ret i64 %intent_result\n"
                ),
                phi,
            )
        }
    };
    let enter = match program {
        Program::Main(_) => "run_main",
        Program::Cycle {
            startup: Some(_), ..
        } => "run_startup",
        Program::Cycle { startup: None, .. } => "cycle",
    };
    format!(
        r#"
define i64 @{symbol}(i32 %abi, i64 %slot, i64 %objects, i64 %properties, i64 %output_limit) "target-cpu"="{cpu}" {{
entry:
  %version = icmp eq i32 %abi, {abi}
  br i1 %version, label %open, label %mismatch
mismatch:
  ret i64 6
open:
  %session = call i64 @zeb_objects_call(i32 0, i64 0, i64 %objects, i64 %properties, i64 %output_limit)
  %open_status = call i32 @zeb_objects_status()
  %opened = icmp eq i32 %open_status, 0
  br i1 %opened, label %initialize, label %open_error
open_error:
  %open_wide = zext i32 %open_status to i64
  %open_bits = shl i64 %open_wide, 8
  %open_result = or i64 %open_bits, 8
  ret i64 %open_result
initialize:
  call i64 @zeb_objects_call(i32 139, i64 %session, i64 8, i64 %slot, i64 0)
{select_model}  %tables_outcome = call %out @zeb_install_tables()
  %tables_code = extractvalue %out %tables_outcome, 1
  %tables_failed = icmp ne i32 %tables_code, 0
  br i1 %tables_failed, label %tables_error, label %construct
tables_error:
  ret i64 20
construct:
  %init_status = call i32 @zeb_initialize_objects(i64 %session)
  %initialized = icmp eq i32 %init_status, 0
  br i1 %initialized, label %invoke, label %init_error
init_error:
  %init_wide = zext i32 %init_status to i64
  %init_bits = shl i64 %init_wide, 8
  %init_result = or i64 %init_bits, 4
  ret i64 %init_result
invoke:
  %static_outcome = call %out @zeb_run_static_initializers()
  %static_code = extractvalue %out %static_outcome, 1
  %static_failed = icmp ne i32 %static_code, 0
  br i1 %static_failed, label %finish, label %preinit
preinit:
  %preinit_outcome = call %out @zeb_run_preinit()
  %preinit_code = extractvalue %out %preinit_outcome, 1
  %preinit_failed = icmp ne i32 %preinit_code, 0
  br i1 %preinit_failed, label %finish, label %{enter}
{body}finish:
  %outcome = phi %out {phi}
  %value = extractvalue %out %outcome, 0
  %code = extractvalue %out %outcome, 1
  %site = extractvalue %out %outcome, 2
  %value_tag = trunc i128 %value to i32
  %owned_text = icmp eq i32 %value_tag, 7
  %no_error = icmp eq i32 %code, 0
  %copy_text = and i1 %owned_text, %no_error
  br i1 %copy_text, label %copy_result, label %close_result
copy_result:
  %value_bits = lshr i128 %value, 32
  %text_handle = trunc i128 %value_bits to i64
  %copied = call i64 @zeb_objects_call(i32 28, i64 %session, i64 %text_handle, i64 0, i64 0)
  %copy_status = call i32 @zeb_objects_status()
  br label %close_result
close_result:
  %output_status = phi i32 [ 0, %finish ], [ %copy_status, %copy_result ]
  %closed = call i64 @zeb_objects_call(i32 9, i64 %session, i64 0, i64 0, i64 0)
  %output_failed = icmp ne i32 %output_status, 0
  br i1 %output_failed, label %output_error, label %classify_result
output_error:
  %output_wide = zext i32 %output_status to i64
  %output_bits = shl i64 %output_wide, 8
  %output_error_result = or i64 %output_bits, 4
  ret i64 %output_error_result
classify_result:
  %failed = icmp ne i32 %code, 0
  br i1 %failed, label %source_error, label %success
source_error:
  %code_wide = zext i32 %code to i64
  %code_bits = shl i64 %code_wide, 8
  %site_bits = shl i64 %site, 32
  %error_bits = or i64 %code_bits, %site_bits
  %error_result = or i64 %error_bits, 3
  ret i64 %error_result
success:
  %tag = trunc i128 %value to i32
  %supported = icmp ule i32 %tag, 2
  %scalar = trunc i128 %value to i64
  %scalar_result = select i1 %supported, i64 %scalar, i64 5
  %is_text = icmp eq i32 %tag, 4
  %text_bits = and i64 %scalar, -4294967296
  %text_result = or i64 %text_bits, 7
  %result = select i1 %is_text, i64 %text_result, i64 %scalar_result
  %final_result = select i1 %owned_text, i64 9, i64 %result
  ret i64 %final_result
}}
define i32 @{symbol}_output_byte(i64 %slot, i64 %offset) "target-cpu"="{cpu}" {{
  %byte = call i32 @zeb_host_output_byte(i64 %slot, i64 %offset)
  ret i32 %byte
}}
define i64 @{symbol}_output_len(i64 %slot) "target-cpu"="{cpu}" {{
  %len = call i64 @zeb_host_output_len(i64 %slot)
  ret i64 %len
}}
define i64 @{symbol}_output_word(i64 %slot, i64 %offset) "target-cpu"="{cpu}" {{
  %word = call i64 @zeb_host_output_word(i64 %slot, i64 %offset)
  ret i64 %word
}}
define i64 @{symbol}_event_len(i64 %slot) "target-cpu"="{cpu}" {{
  %len = call i64 @zeb_host_event_len(i64 %slot)
  ret i64 %len
}}
define i64 @{symbol}_event_word(i64 %slot, i64 %offset) "target-cpu"="{cpu}" {{
  %word = call i64 @zeb_host_event_word(i64 %slot, i64 %offset)
  ret i64 %word
}}
define i32 @{symbol}_text_byte(i32 %literal, i64 %offset) "target-cpu"="{cpu}" {{
  %byte = call i32 @zeb_objects_text_byte(i32 %literal, i64 %offset)
  ret i32 %byte
}}
define i32 @{symbol}_poll(i64 %slot) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_poll(i64 %slot)
  ret i32 %code
}}
define i32 @{symbol}_drained(i64 %slot) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_drained(i64 %slot)
  ret i32 %code
}}
define i32 @{symbol}_reply_byte(i64 %slot, i32 %byte) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_reply_byte(i64 %slot, i32 %byte)
  ret i32 %code
}}
define i32 @{symbol}_reply_action(i64 %slot, i32 %verb) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_reply_action(i64 %slot, i32 %verb)
  ret i32 %code
}}
define i32 @{symbol}_reply_value(i64 %slot, i32 %tag, i64 %payload) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_reply_value(i64 %slot, i32 %tag, i64 %payload)
  ret i32 %code
}}
define i32 @{symbol}_reply_subject(i64 %slot, i64 %handle) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_reply_subject(i64 %slot, i64 %handle)
  ret i32 %code
}}
define i32 @{symbol}_resume(i64 %slot) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_resume(i64 %slot)
  ret i32 %code
}}
define i32 @{symbol}_close(i64 %slot) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_close(i64 %slot)
  ret i32 %code
}}
define i32 @{symbol}_reset(i64 %slot) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_reset(i64 %slot)
  ret i32 %code
}}
define i32 @{symbol}_discard(i64 %slot) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_discard(i64 %slot)
  ret i32 %code
}}
define i32 @{symbol}_finish(i64 %slot, i64 %outcome) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_finish(i64 %slot, i64 %outcome)
  ret i32 %code
}}
define i64 @{symbol}_outcome(i64 %slot) "target-cpu"="{cpu}" {{
  %value = call i64 @zeb_host_outcome(i64 %slot)
  ret i64 %value
}}
define i64 @{symbol}_persistence(i64 %slot, i32 %op, i64 %value) "target-cpu"="{cpu}" {{
  %result = call i64 @zeb_host_persistence(i64 %slot, i32 %op, i64 %value)
  ret i64 %result
}}
define i32 @{symbol}_inspect(i64 %slot, i32 %kind, i64 %a, i64 %b) "target-cpu"="{cpu}" {{
  %code = call i32 @zeb_host_inspect(i64 %slot, i32 %kind, i64 %a, i64 %b)
  ret i32 %code
}}
define i64 @{symbol}_result_len(i64 %slot) "target-cpu"="{cpu}" {{
  %len = call i64 @zeb_host_result_len(i64 %slot)
  ret i64 %len
}}
define i64 @{symbol}_result(i64 %slot, i64 %index) "target-cpu"="{cpu}" {{
  %value = call i64 @zeb_host_result(i64 %slot, i64 %index)
  ret i64 %value
}}

"#
    )
}

/// The generated C host.
///
/// the call names come from the manifest's export table rather than
/// from `{symbol}_whatever` written out by hand, so a consumer cannot call
/// something the bundle does not declare. Removing an export fails the
/// build here instead of failing the C compiler later with a name nobody can
/// trace back to a list.
fn consumer(described: &manifest::Manifest) -> Result<String, String> {
    let symbol = &described.symbol;
    // The version the entry will accept, read from the same description the
    // header is rendered from rather than written again here.
    let abi = described.abi;
    let call = |suffix: &str| -> Result<String, String> {
        if manifest::EXPORTS.iter().any(|e| e.suffix == suffix) {
            Ok(format!("{symbol}{suffix}"))
        } else {
            Err(format!(
                "the consumer calls {suffix}, which the bundle does not export"
            ))
        }
    };
    // Named once each, so a missing export is reported before any text is built.
    for suffix in [
        "",
        "_reset",
        "_discard",
        "_poll",
        "_output_byte",
        "_output_len",
        "_output_word",
        "_event_len",
        "_event_word",
        "_drained",
        "_reply_byte",
        "_reply_action",
        "_reply_subject",
        "_reply_value",
        "_resume",
        "_close",
        "_finish",
        "_outcome",
        "_inspect",
        "_persistence",
        "_result_len",
        "_result",
        "_text_byte",
    ] {
        call(suffix)?;
    }
    // the host drives the cycle. The game runs on its own thread and
    // suspends when it wants input; this loop never blocks on the game and the
    // game never reads the console. The thread is created here rather than in
    // the runtime, which forbids unsafe and so cannot call back into the game.
    Ok(format!(
        r#"#include "zeb_game.h"
#include <inttypes.h>
#ifdef _WIN32
#include <windows.h>
#include <process.h>
#else
#include <pthread.h>
#endif
#include <stdio.h>
#include <stdlib.h>
#include <errno.h>
#include <string.h>
#define SLOT UINT64_C(1)
static uint64_t objects = 100000, properties = 1000000, output_limit = 8388608;
static int quota(const char *text, uint64_t *value) {{
    if (!*text) return 0;
    for (const char *p = text; *p; ++p) if (*p < '0' || *p > '9') return 0;
    char *end; errno = 0;
    uintmax_t parsed = strtoumax(text, &end, 10);
    if (errno || *end || parsed > UINT64_MAX) return 0;
    *value = (uint64_t)parsed; return 1;
}}
/* Events are what an engine binds assets to; a console prints them only when
   asked, so a transcript stays readable. ZEB_EVENTS=1 turns them on. */
static int SHOW_EVENTS = 0;
#ifdef _WIN32
static unsigned __stdcall worker(void *ignored) {{
#else
static void *worker(void *ignored) {{
#endif
    (void)ignored;
    uint64_t result = {symbol}({abi}, SLOT, objects, properties, output_limit);
    {symbol}_finish(SLOT, result);
    return 0;
}}
/* Print the semantic events a turn produced, beside its text ().

   A record is self-describing, so nothing has to be agreed in advance:
   a word of length, a word of id length, the id padded to a whole number of
   words, a word of how many entities it concerns, then a handle each. A real
   host looks the id up in its asset table; a console prints it. */
static void events(void) {{
    uint64_t length = {symbol}_event_len(SLOT);
    if (SHOW_EVENTS == 2) {{
        fputs("@events ", stdout);
        for (uint64_t i = 0; i < length; ++i) {{
            uint64_t w = {symbol}_event_word(SLOT, i & ~(uint64_t)7);
            printf("%02x", (unsigned)((w >> (8 * (i & 7))) & 255));
        }}
        putchar('\n'); fflush(stdout); return;
    }}
    for (uint64_t at = 0; at + 8 <= length;) {{
        uint64_t words = {symbol}_event_word(SLOT, at);
        if (words == 0) break;
        uint64_t id_len = {symbol}_event_word(SLOT, at + 8);
        printf("{{event ");
        for (uint64_t step = 0; step < id_len; ++step) {{
            uint64_t word = {symbol}_event_word(SLOT, at + 16 + (step & ~(uint64_t)7));
            putchar((int)((word >> (8 * (step & 7))) & 0xff));
        }}
        uint64_t padded = ((id_len + 7) / 8) * 8;
        uint64_t count = {symbol}_event_word(SLOT, at + 16 + padded);
        for (uint64_t step = 0; step < count; ++step) {{
            printf(" %" PRIu64, {symbol}_event_word(SLOT, at + 24 + padded + step * 8));
        }}
        printf("}}\n");
        at += words * 8;
    }}
    fflush(stdout);
}}
/* Print everything the game handed over, eight bytes at a time. */
static int drain(int *emitted) {{
    uint64_t length = {symbol}_output_len(SLOT);
    for (uint64_t offset = 0; offset < length; offset += 8) {{
        uint64_t word = {symbol}_output_word(SLOT, offset);
        for (uint64_t step = 0; step < 8 && offset + step < length; ++step) {{
            int byte = (int)((word >> (8 * step)) & 0xff);
            if (SHOW_EVENTS != 2 && putchar(byte) == EOF) return 0;
            *emitted = 1;
        }}
    }}
    if (SHOW_EVENTS) events();
    {symbol}_drained(SLOT);
    return fflush(stdout) == 0;
}}
/* Read one line of the script. Returns 0 at end of input. */
static int line(char *buffer, size_t limit) {{
    size_t used = 0;
    int c = getchar();
    if (c == EOF) return 0;
    while (c != EOF && c != '\n') {{
        if (c != '\r' && used + 1 < limit) buffer[used++] = (char)c;
        c = getchar();
    }}
    buffer[used] = '\0';
    return 1;
}}
/* Print what the last inspection answered (). */
static void results(void) {{
    uint64_t count = {symbol}_result_len(SLOT);
    printf("[%" PRIu64 " result(s)", count);
    for (uint64_t i = 0; i < count; ++i) printf(" %" PRIu64, {symbol}_result(SLOT, i));
    printf("]\n");
    fflush(stdout);
}}
/* Answer one request. A plain line is a parser intent; "!verb a b" is an action
   intent, which is the shape a CRPG host submits (); "?kind a b" asks
   for an inspection, answered at the game's next safe boundary (). */
static int answer(void) {{
    char buffer[1024];
    if (!line(buffer, sizeof buffer)) return {symbol}_close(SLOT) == 0;
    if (strncmp(buffer, ":save ", 6) == 0 || strncmp(buffer, ":restore ", 9) == 0) {{
        int restore = buffer[1] == 'r';
        char *path = buffer + (restore ? 9 : 6);
        path[strcspn(path, "\r\n")] = 0;
        uint64_t code = {symbol}_persistence(SLOT, 0, 0);
        if (restore && code == 0) {{
            FILE *file = fopen(path, "rb");
            if (!file) {{ perror(path); return answer(); }}
            int byte;
            while ((byte = fgetc(file)) != EOF && code == 0)
                code = {symbol}_persistence(SLOT, 1, (uint64_t)byte);
            if (ferror(file)) code = 11;
            fclose(file);
        }}
        if (code == 0) code = {symbol}_persistence(SLOT, restore ? 3 : 2, 0);
        if (code == 0) {{
            if ({symbol}_poll(SLOT) != 0) return 0;
            code = {symbol}_persistence(SLOT, 4, 0);
        }}
        if (!restore && code == 0) {{
            FILE *file = fopen(path, "wb");
            if (!file) {{ perror(path); return answer(); }}
            uint64_t length = {symbol}_persistence(SLOT, 5, 0);
            for (uint64_t i = 0; i < length; ++i)
                if (fputc((int){symbol}_persistence(SLOT, 6, i), file) == EOF) {{ code=11; break; }}
            if (fclose(file) != 0) code=11;
        }}
        printf("[%s %s code=%" PRIu64 "]\n", restore ? "restore" : "save", code ? "failed" : "ok", code);
        return answer();
    }}
    if (buffer[0] == '?') {{
        unsigned kind = 0; unsigned long long a = 0, b = 0;
        sscanf(buffer + 1, "%u %llu %llu", &kind, &a, &b);
        /* A turn answers one inspection, and a second over an unanswered one
           is refused rather than replacing it (). Say which, because a
           host that loses a query silently binds the wrong handle to a name
           and never finds out — which is the bug the refusal exists to stop. */
        uint32_t asked = {symbol}_inspect(SLOT, kind, (uint64_t)a, (uint64_t)b);
        if (asked != 0) {{
            fprintf(stderr, asked == 10
                ? "an inspection is already waiting; one turn answers one\n"
                : "the inspection was refused (%" PRIu32 ")\n", asked);
            return 0;
        }}
        /* Nothing was consumed from the game's point of view; ask again. */
        return answer();
    }}
    if (buffer[0] == '#') {{
        /* print a literal by index. A presentation answer gives a name
           as a literal, and this is how a host turns it into characters. */
        unsigned literal = 0;
        if (sscanf(buffer + 1, "%u", &literal) != 1) return 0;
        printf("[literal ");
        for (uint64_t offset = 0;; ++offset) {{
            uint32_t byte = {symbol}_text_byte(literal, offset);
            if (byte > 255) break;
            putchar((int)byte);
        }}
        printf("]\n");
        fflush(stdout);
        return answer();
    }}
    /* Typed action protocol for example hosts: @verb i:number s:UTF8_HEX h:handle.
       This is transport, never player prose or a dialogue parser. */
    if (buffer[0] == '@') {{
        unsigned verb = 0; int used = 0;
        if (sscanf(buffer + 1, "%u%n", &verb, &used) != 1) return 0;
        if ({symbol}_reply_action(SLOT, verb)) return 0;
        char *part = strtok(buffer + 1 + used, " \r\n");
        while (part) {{
            if (part[1] != ':') return 0;
            if (part[0] == 's') {{
                const char *hex = part + 2;
                while (*hex) {{
                    unsigned byte = 0; int step = 0;
                    if (!hex[1] || sscanf(hex, "%2x%n", &byte, &step) != 1 || step != 2) return 0;
                    if ({symbol}_reply_byte(SLOT, byte)) return 0;
                    hex += 2;
                }}
                if ({symbol}_reply_value(SLOT, 4, 0)) return 0;
            }} else if (part[0] == 'i') {{
                char *end; errno = 0; long long n = strtoll(part + 2, &end, 10);
                if (errno || *end || n < INT32_MIN || n > INT32_MAX) return 0;
                if ({symbol}_reply_value(SLOT, 2, (uint64_t)n)) return 0;
            }} else if (part[0] == 'h') {{
                uint64_t h = 0; if (!quota(part + 2, &h)) return 0;
                if ({symbol}_reply_subject(SLOT, h)) return 0;
            }} else return 0;
            part = strtok(NULL, " \r\n");
        }}
        return {symbol}_resume(SLOT) == 0;
    }}
    if (buffer[0] == '!') {{
        unsigned verb = 0; int used = 0;
        if (sscanf(buffer + 1, "%u%n", &verb, &used) != 1) return 0;
        if ({symbol}_reply_action(SLOT, verb) != 0) return 0;
        const char *rest = buffer + 1 + used;
        while (*rest) {{
            unsigned long long handle = 0; int step = 0;
            if (sscanf(rest, " %llu%n", &handle, &step) != 1) break;
            if ({symbol}_reply_subject(SLOT, (uint64_t)handle) != 0) return 0;
            rest += step;
        }}
        return {symbol}_resume(SLOT) == 0;
    }}
    for (const char *p = buffer; *p; ++p) {{
        if ({symbol}_reply_byte(SLOT, (uint32_t)(unsigned char)*p) != 0) return 0;
    }}
    return {symbol}_resume(SLOT) == 0;
}}
int main(int argc, char **argv) {{
    if (argc != 1 && (argc != 4 || !quota(argv[1], &objects) || !quota(argv[2], &properties) || !quota(argv[3], &output_limit))) {{
        fputs("usage: consumer [object-limit property-limit output-byte-limit]\n", stderr); return 64;
    }}
    {{ const char *want = getenv("ZEB_EVENTS"); SHOW_EVENTS = want && want[0] == '2' ? 2 : want && want[0] == '1'; }}
    if ({symbol}_reset(SLOT) != 0) {{ fputs("cannot start a session\n", stderr); return 70; }}
#ifdef _WIN32
    uintptr_t thread = _beginthreadex(NULL, 0, worker, NULL, 0, NULL);
    if (!thread) {{
#else
    pthread_t thread;
    if (pthread_create(&thread, NULL, worker, NULL) != 0) {{
#endif
        fputs("cannot start the session worker\n", stderr); return 70;
    }}
    int emitted = 0;
    for (;;) {{
        uint32_t state = {symbol}_poll(SLOT);
        if (state > 4) {{ fprintf(stderr, "session failure %" PRIu32 "\n", state); {symbol}_close(SLOT); break; }}
        if (!drain(&emitted)) return 74;
        if (state == 2) break;
        /* A host action is accepted and an engine query answers zero: a console
           has neither an engine nor anything to animate. */
        if (state == 3 || state == 4) {{
            if ({symbol}_reply_byte(SLOT, (uint32_t)(state == 3 ? '1' : '0')) != 0) return 74;
            if ({symbol}_resume(SLOT) != 0) return 74;
            continue;
        }}
        if (SHOW_EVENTS == 2) {{ puts("@ready"); fflush(stdout); }}
        if (!answer()) return 74;
        if ({symbol}_result_len(SLOT) > 0) results();
    }}
#ifdef _WIN32
    WaitForSingleObject((HANDLE)thread, INFINITE);
    CloseHandle((HANDLE)thread);
#else
    pthread_join(thread, NULL);
#endif
    if (!drain(&emitted)) return 74;
    uint64_t result = {symbol}_outcome(SLOT);
    {symbol}_discard(SLOT);
    uint32_t tag = (uint32_t)result & 255;
    switch (tag) {{
        case 0: if (!emitted) puts("nil"); return 0;
        case 1: puts("true"); return 0;
        case 2: {{
            uint32_t bits = (uint32_t)(result >> 32);
            int64_t value = bits <= INT32_MAX ? (int64_t)bits : (int64_t)bits - INT64_C(4294967296);
            printf("%" PRId64 "\n", value); return 0;
        }}
        case 3: case 4: case 8:
            fprintf(stderr, "%s error %" PRIu32 " at source byte %" PRIu32 "\n",
                tag == 3 ? "source" : "runtime", ((uint32_t)result >> 8), (uint32_t)(result >> 32));
            return 70;
        case 7: {{
            uint32_t literal = (uint32_t)(result >> 32);
            for (uint64_t offset = 0;; ++offset) {{
                uint32_t byte = {symbol}_text_byte(literal, offset);
                if (byte == 256) break;
                if (byte > 255 || putchar((int)byte) == EOF) return 74;
            }}
            putchar('\n'); return 0;
        }}
        case 9: putchar('\n'); return 0;
        case 5: fputs("consumer cannot display an object result\n", stderr); return 70;
        default: fputs("incompatible game ABI\n", stderr); return 70;
    }}
}}
"#
    ))
}

// The same argument recipe is hashed and executed. Placeholder output/install
// names break the digest/name recursion; real paths are remapped in the binary.
fn runtime_args(
    runtime: &str,
    out: &str,
    optimize: bool,
    target: Target,
    tools: &Toolchain,
) -> Vec<String> {
    let mut args = vec![
        "--edition=2024".into(),
        "--remap-path-prefix".into(),
        format!("{out}=zeb-runtime"),
        "--crate-name=zeb_runtime".into(),
        "--crate-type=dylib".into(),
        "--cfg".into(),
        "feature=\"objects\"".into(),
        "--target".into(),
        target.rust_triple().into(),
        "-C".into(),
        format!("target-cpu={}", target.cpu()),
        "-C".into(),
        format!("opt-level={}", if optimize { 2 } else { 0 }),
        "../runtime.rs".into(),
        "-o".into(),
        runtime.into(),
    ];
    args.extend(tools.rust_args(target, runtime));
    args
}

pub fn build(
    ast: &Ast,
    source: &Source,
    out: &Path,
    optimize: bool,
    cache: Option<&Path>,
) -> Result<(), String> {
    let host = Target::host()?;
    if source.byte_len() > u32::MAX as usize {
        return Err("object consumer requires source offsets within u32".to_owned());
    }
    let declared = |wanted: &str, arity: usize| {
        ast.functions
            .iter()
            .enumerate()
            .find_map(|(i, id)| match &ast.nodes[id.0].syntax {
                Syntax::Function {
                    name, parameters, ..
                } if name == wanted && parameters.len() == arity => Some(i),
                _ => None,
            })
    };
    let program = if ast.lifetimes {
        // the author writes a turn, not a loop. The cycle, the prompt
        // and the tokenizer all belong to the host side of the boundary.
        //
        // At least one of the text or structured-action entry points is required.
        let turn = declared("turn", 1);
        let act = declared("act", 2);
        if turn.is_none() && act.is_none() {
            return Err(
                "a lifetimes bundle requires turn(tokens) or act(verb, subjects)".to_owned(),
            );
        }
        let token = zeb_frontend::sema::enumerator(ast, "tokWord")
            .ok_or("the command cycle requires `enum token tokWord;`")?;
        Program::Cycle {
            startup: declared("startup", 0),
            turn,
            recover: declared("recover", 1),
            act,
            token,
        }
    } else {
        Program::Main(
            declared("main", 0).ok_or("object bundle requires main() with no parameters")?,
        )
    };
    let modules = host
        .slices()
        .iter()
        .copied()
        .map(|target| {
            llvm::emit_objects(ast, target)
                .map_err(|d| format!("{} at byte {}: {}", d.code, d.byte, d.message))
        })
        .collect::<Result<Vec<_>, _>>()?;
    fs::create_dir(out).map_err(|e| format!("output directory must be new: {e}"))?;
    let out = out.canonicalize().map_err(|e| e.to_string())?;
    let result = (|| {
        let tools = Toolchain::resolve(&out)?;
        tools.record(&out)?;
        let rustc = tools.rustc.clone();
        write(&out, "source.t", source.original_bytes())?;
        write(
            &out,
            "source-encoding.txt",
            format!("{:?}\n", source.encoding),
        )?;
        write(&out, "runtime.rs", RUNTIME)?;
        write(&out, "objects.rs", OBJECTS)?;
        fs::create_dir_all(out.join("objects")).map_err(|e| e.to_string())?;
        write(&out, "objects/history.rs", HISTORY)?;
        write(&out, "objects/persistence.rs", PERSISTENCE)?;
        write(&out, "save.rs", SAVE)?;
        write(&out, "native_objects.rs", NATIVE)?;
        write(&out, "output.rs", OUTPUT)?;
        write(&out, "sort.rs", SORT)?;
        write(&out, "regex.rs", REGEX)?;
        write(&out, "relations.rs", RELATIONS)?;
        write(&out, "session.rs", SESSION)?;
        write(&out, "grammar.rs", GRAMMAR)?;
        write(&out, "tables.rs", TABLES)?;
        /*
         * Two identities now, not one .
         *
         * The **bundle** is what it always was: the emitted module, the
         * options, and the runtime it runs against. The **runtime** is only
         * itself — its own sources and the options it was built with — because
         * program-specific tables are carried by the game module. Two programs built with the
         * same tools and options therefore name the same runtime library, and
         * it is the same library, byte for byte, which is what makes it worth
         * building once.
         */
        let recipes = host
            .slices()
            .iter()
            .map(|&target| runtime_args("RUNTIME", "OUTPUT", optimize, target, &tools).join("\n"))
            .collect::<Vec<_>>()
            .join("\n");
        write(
            &out,
            "runtime-identity.txt",
            zebc::runtime_cache::identity(&[
                PROFILE,
                &runtime_identity(),
                &tools.lock_json,
                &tools.profile_json,
                &recipes,
                include_str!("platform.rs"),
                &tools.rustc,
            ]),
        )?;
        let runtime_digest = zebc::digest::file(&out.join("runtime-identity.txt"))?;
        write(
            &out,
            "identity.txt",
            format!(
                "{PROFILE}\noptimize={optimize}\n{}\n{runtime_digest}",
                modules[0]
            ),
        )?;
        let identity = zebc::digest::file(&out.join("identity.txt"))?;
        let identity = identity.as_str();
        if identity.len() != 64 || !identity.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid bundle digest".to_owned());
        }
        if runtime_digest.len() != 64 || !runtime_digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid runtime digest".to_owned());
        }
        // the bundle describes its own surface, and the header is
        // rendered from that description rather than written beside it. A name
        // that appears in one appears in both or in neither.
        let mut described = manifest::describe(
            ast,
            identity,
            &runtime_digest,
            optimize,
            match &program {
                Program::Cycle {
                    startup,
                    turn,
                    act,
                    recover,
                    token,
                } => manifest::Entry {
                    startup: startup.is_some(),
                    turn: turn.is_some(),
                    act: act.is_some(),
                    recover: recover.is_some(),
                    token: Some(*token),
                },
                Program::Main(_) => manifest::Entry {
                    startup: false,
                    turn: false,
                    act: false,
                    recover: false,
                    token: None,
                },
            },
            PROFILE,
            ABI,
        );
        described.set_target(host);
        manifest::validate(&described)?;
        let exported = described.symbol.clone();
        let game = described.game.clone();
        let runtime = described.runtime.clone();
        write(&out, "manifest.json", described.json().render())?;
        write(&out, "zeb_game.h", described.header())?;
        write(&out, "consumer.c", consumer(&described)?)?;
        let consumer_name = host.executable("consumer");
        for (&target, mut module) in host.slices().iter().zip(modules) {
            let arch = target.arch();
            let dir = out.join(arch);
            fs::create_dir(&dir).map_err(|e| e.to_string())?;
            let cached = cache.map(|root| root.join(&runtime_digest).join(arch));
            let hit = match &cached {
                Some(path) => zebc::runtime_cache::restore(path, &dir, &runtime)?,
                None => false,
            };
            if !hit {
                let rust_log = tools.run(
                    &dir,
                    &rustc,
                    &runtime_args(&runtime, &out.to_string_lossy(), optimize, target, &tools),
                )?;
                write(&dir, "rust-build.txt", rust_log)?;
                if let Some(path) = &cached {
                    zebc::runtime_cache::publish(path, &dir, &runtime)?;
                }
            }
            if cache.is_some() {
                eprintln!("runtime cache {arch}: {}", if hit { "hit" } else { "miss" });
            }
            let names = tools.symbols(&dir, &runtime, false)?;
            let readable = tools.symbols(&dir, &runtime, true)?;
            module += "\ndeclare i32 @zeb_objects_text_byte(i32, i64)\ndeclare i32 @zeb_objects_output_byte(i64)\ndeclare i32 @zeb_host_reset(i64)\ndeclare i32 @zeb_host_discard(i64)\ndeclare i32 @zeb_host_poll(i64)\ndeclare i32 @zeb_host_output_byte(i64, i64)\ndeclare i64 @zeb_host_output_len(i64)\ndeclare i64 @zeb_host_output_word(i64, i64)\ndeclare i64 @zeb_host_event_len(i64)\ndeclare i64 @zeb_host_event_word(i64, i64)\ndeclare i32 @zeb_host_drained(i64)\ndeclare i32 @zeb_host_reply_byte(i64, i32)\ndeclare i32 @zeb_host_reply_action(i64, i32)\ndeclare i32 @zeb_host_reply_value(i64, i32, i64)\ndeclare i32 @zeb_host_reply_subject(i64, i64)\ndeclare i32 @zeb_host_resume(i64)\ndeclare i32 @zeb_host_close(i64)\ndeclare i32 @zeb_host_finish(i64, i64)\ndeclare i64 @zeb_host_outcome(i64)\ndeclare i32 @zeb_host_inspect(i64, i32, i64, i64)\ndeclare i64 @zeb_host_result_len(i64)\ndeclare i64 @zeb_host_result(i64, i64)\n";
            module += "\ndeclare i64 @zeb_host_persistence(i64, i32, i64)\n";
            let mut boundary = entry(&exported, &program, target.cpu(), ABI);
            let mut schema_calls = String::new();
            for (i, chunk) in identity.as_bytes().as_chunks::<16>().0.iter().enumerate() {
                let hex = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
                let word = u64::from_str_radix(hex, 16)
                    .map_err(|e| e.to_string())?
                    .swap_bytes();
                schema_calls += &format!(
                    "  call i64 @zeb_objects_call(i32 151, i64 %session, i64 {i}, i64 {word}, i64 0)\n"
                );
            }
            boundary = boundary.replace("construct:\n", &format!("construct:\n{schema_calls}"));
            module += &boundary;
            for method in [
                "call",
                "grammar_call",
                "regex_call",
                "dictionary_call",
                "lookup_call",
                "status",
                "kind",
                "session",
                "text_byte",
                "output_byte",
            ] {
                let name = symbol(&names, &readable, method, target)?;
                module = module.replace(&format!("@zeb_objects_{method}"), &format!("@{name}"));
            }
            for method in [
                "host_reset",
                "host_discard",
                "host_poll",
                "host_output_byte",
                "host_output_len",
                "host_output_word",
                "host_event_len",
                "host_event_word",
                "host_drained",
                "host_reply_byte",
                "host_reply_action",
                "host_reply_subject",
                "host_reply_value",
                "host_resume",
                "host_close",
                "host_finish",
                "host_outcome",
                "host_inspect",
                "host_persistence",
                "host_result_len",
                "host_result",
            ] {
                let name = symbol(&names, &readable, method, target)?;
                module = module.replace(&format!("@zeb_{method}"), &format!("@{name}"));
            }
            if target.is_windows() {
                // Every external declaration here is a runtime function, not an LLVM intrinsic.
                module = module
                    .replace("declare i32 @", "declare dllimport i32 @")
                    .replace("declare i64 @", "declare dllimport i64 @");
            }
            write(&dir, "game.ll", zebc::platform::export_ir(module, target))?;
            let mut args = tools.shared_args(target, &game);
            args.extend([
                if optimize { "-O2".into() } else { "-O0".into() },
                "game.ll".into(),
                tools.import_library(&runtime),
                "-o".into(),
                game.clone(),
            ]);
            let log = tools.clang(&dir, target, &args)?;
            write(&dir, "game-build.txt", log)?;
            let mut args: Vec<String> =
                ["-std=c11", "-Wall", "-Wextra", "-Werror", "../consumer.c"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
            args.extend([
                tools.import_library(&game),
                "-o".into(),
                consumer_name.clone(),
            ]);
            if !target.is_windows() {
                args.push("-pthread".into());
            }
            args.extend(tools.loader_args(target));
            args.extend(tools.linker_args(target));
            let log = tools.clang(&dir, target, &args)?;
            write(&dir, "consumer-build.txt", log)?;
            write(
                &dir,
                "dependencies.txt",
                tools.dependencies(&dir, &[&game, &runtime, &consumer_name])?,
            )?;
        }
        tools.assemble(&out, &[&game, &runtime, &consumer_name])?;
        // the manifest may not name a function the bundle does not
        // define. A description that is merely plausible is worse than none —
        // a binding generator reads it and produces code that links against
        // nothing. So it is checked against the built library's own symbols,
        // read as data rather than as a log.
        let symbols = tools.exports(&out, &game)?;
        write(&out, "manifest-symbols.txt", &symbols)?;
        let missing = manifest::missing_exports(&described, &symbols);
        if !missing.is_empty() {
            return Err(format!(
                "the manifest names {} function(s) the bundle does not export: {}",
                missing.len(),
                missing.join(", ")
            ));
        }
        write(
            &out,
            "BUILD.txt",
            format!(
                "built\nprofile={PROFILE}\ngame={game}\nruntime={runtime}\nconsumer={consumer_name}\nmanifest=manifest.json\nexecution=not-run-by-builder\nqualification=not-claimed\n"
            ),
        )?;
        Ok(())
    })();
    if let Err(error) = &result {
        let _ = write(&out, "FAILED.txt", error);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{Program, entry};

    fn cycle(turn: Option<usize>, act: Option<usize>) -> String {
        entry(
            "game",
            &Program::Cycle {
                startup: None,
                turn,
                recover: None,
                act,
                token: 4,
            },
            "apple-m1",
            super::ABI,
        )
    }

    /// The emitted entry must accept the version published by its manifest.
    #[test]
    fn the_entry_accepts_the_version_the_bundle_publishes() {
        let emitted = cycle(Some(11), None);
        assert!(
            emitted.contains(&format!("%version = icmp eq i32 %abi, {}", super::ABI)),
            "the entry checks a version other than {}:\n{}",
            super::ABI,
            emitted
                .lines()
                .find(|line| line.contains("%version ="))
                .unwrap_or("(no version check at all)")
        );
        // And the symbol the header and the manifest name carries it too.
        assert!(super::PROFILE.ends_with(&super::ABI.to_string()));
    }

    /// the two ways in are independent. A game declaring one gets a
    /// block that calls it and a block that quietly ends the run for the other,
    /// so an intent the game cannot take never reaches a function that is not
    /// there.
    #[test]
    fn each_entry_point_is_emitted_only_when_it_is_declared() {
        let lines_only = cycle(Some(11), None);
        assert!(lines_only.contains("@zfn11(i128 %tokens_value)"));
        assert!(lines_only.contains("run_act:\n  br label %quiet"));
        assert!(!lines_only.contains("%act_outcome"));

        let actions_only = cycle(None, Some(22));
        assert!(actions_only.contains("@zfn22(i128 %verb_value, i128 %subjects_value)"));
        assert!(actions_only.contains("run_turn:\n  br label %quiet"));
        assert!(!actions_only.contains("%turn_outcome"));

        let both = cycle(Some(11), Some(22));
        assert!(both.contains("@zfn11(i128 %tokens_value)"));
        assert!(both.contains("@zfn22(i128 %verb_value, i128 %subjects_value)"));
        assert!(!both.contains("br label %quiet\nrun_turn"));
    }

    /// Every block the cycle can branch to has to exist, whichever entry points
    /// are declared: a missing label is a module LLVM refuses to parse, and the
    /// quiet path is reached from both directions.
    #[test]
    fn every_branch_target_is_defined() {
        for (turn, act) in [
            (Some(1usize), Some(2usize)),
            (Some(1), None),
            (None, Some(2)),
        ] {
            let text = cycle(turn, act);
            for label in ["run_turn", "run_act", "end_turn", "quiet", "cycle"] {
                assert!(
                    text.contains(&format!("\n{label}:")),
                    "{label} is branched to but not defined for turn={turn:?} act={act:?}"
                );
            }
        }
    }
}
