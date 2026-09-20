//! the host drives the command cycle and the runtime never blocks it.
//!
//! The game runs on a session worker thread; the host runs its own loop. This
//! module is the entire handoff between them, and the only state they share.
//! The object store stays thread-local on the worker, so nothing here touches a
//! game value: the worker hands over finished bytes and waits for a reply.
//!
//! The worker thread is started by the consumer, not here. Calling generated
//! code from Rust would need an `extern` block, and this crate forbids unsafe.
//!
//! Sessions are keyed, so one process can run several independently.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

pub const PACKET_LIMIT: usize = 65_536;
static NEXT_SNAPSHOT: AtomicU32 = AtomicU32::new(1);

fn next_snapshot(counter: &AtomicU32) -> Result<u32, u32> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            (n < i32::MAX as u32).then(|| n + 1)
        })
        .map_err(|_| 18u32)
}

/// Copied host values, never Rust layouts or game-owned pointers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scalar {
    Nil,
    True,
    Integer(i32),
    Entity(u64),
    Text(Vec<u8>),
}

/// What the game is waiting for. The host services one and resumes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Request {
    /// Input at a completed command boundary; permits persistence.
    Command,
    /// A line of input, as `inputLine` and the command cycle ask for.
    Line,
    /// A single key, as `inputKey` asks for.
    Key,
    /// Something only the host can do — play the movement, run the animation —
    /// which it accepts or refuses before the world advances.
    HostAction,
    /// Something only the host knows, answered with a number.
    EngineQuery,
}

impl Request {
    /// The code the host reads from a poll.
    pub fn code(self) -> u32 {
        match self {
            Request::Line | Request::Command => 0,
            Request::Key => 1,
            Request::HostAction => 3,
            Request::EngineQuery => 4,
        }
    }
}

/// What the host sent back. A line intent and an action intent are two shapes
/// of the same thing: a parser front end is one way to produce one.
#[derive(Clone, Debug)]
pub enum Reply {
    Persistence {
        restore: bool,
        bytes: Vec<u8>,
    },
    /// Text, or the end of input.
    Line(Option<String>),
    /// An action the host chose directly, with the entities it names.
    Action {
        verb: u32,
        subjects: Vec<Scalar>,
    },
    /// Whether a host action was accepted.
    Accepted(bool),
    /// The answer to an engine query.
    Value(i64),
}

#[derive(Default)]
struct Shared {
    /// Set by the worker when it suspends, cleared when the host resumes it.
    request: Option<Request>,
    /// What the host has sent back, waiting for the worker to take it.
    reply: Option<Reply>,
    /// The verb, subjects and text the host is assembling for its reply.
    pending_text: Vec<u8>,
    save_bytes: Vec<u8>,
    save_status: u64,
    save_busy: bool,
    pending_verb: Option<u32>,
    pending_subjects: Vec<Scalar>,
    pending_size: usize,
    snapshot: u32,
    event_values: Option<usize>,
    /// Output handed over at the last suspension, for the host to drain.
    output: Vec<u8>,
    /// Semantic events a turn produced, for the host to read beside the text.
    /// Records are self-describing, so no table has to be shared:
    ///
    /// ```text
    /// u64 words in this record, including this one
    /// u64 length of the id in bytes
    ///... the id, zero-padded to a whole number of words
    /// u64 how many entities the event concerns
    ///... one u64 handle each
    /// ```
    ///
    /// Little-endian, like the output stream, and drained with it.
    events: Vec<u8>,
    /// Results of the last inspection query, for the host to read.
    results: Vec<u64>,
    /// Set by the host when its input is exhausted.
    closed: bool,
    /// An inspection the host has asked for, serviced by the worker at its next
    /// safe boundary, which is between cycles.
    inspection: Option<(u32, u64, u64)>,
    /// Set once the worker's session has returned, with its outcome.
    finished: Option<u64>,
}

#[derive(Default)]
struct Channel {
    sessions: Mutex<HashMap<u64, Shared>>,
    changed: Condvar,
}

fn channel() -> &'static Channel {
    static CHANNEL: OnceLock<Channel> = OnceLock::new();
    CHANNEL.get_or_init(Channel::default)
}

/// A poisoned lock means the other thread panicked mid-handoff. Report it as a
/// terminal failure rather than propagating the panic across the boundary.
fn locked<T>(guard: Result<T, std::sync::PoisonError<T>>) -> Result<T, u32> {
    guard.map_err(|_| 8u32)
}

/// Run `act` against one session's handoff, creating it if this is its first use.
fn with<T>(slot: u64, act: impl FnOnce(&mut Shared) -> Result<T, u32>) -> Result<T, u32> {
    let channel = channel();
    let mut sessions = locked(channel.sessions.lock())?;
    sessions.try_reserve(1).map_err(|_| 6u32)?;
    let shared = sessions.entry(slot).or_default();
    let result = act(shared);
    drop(sessions);
    channel.changed.notify_all();
    result
}

/// Start a session's handoff, forgetting anything a previous session left.
pub fn reset(slot: u64) -> Result<(), u32> {
    with(slot, |shared| {
        *shared = Shared::default();
        Ok(())
    })
}

/// Forget a session entirely, so a process running many does not accumulate them.
pub fn discard(slot: u64) -> Result<(), u32> {
    let channel = channel();
    let mut sessions = locked(channel.sessions.lock())?;
    sessions.remove(&slot);
    drop(sessions);
    channel.changed.notify_all();
    Ok(())
}

// --- the worker side: called from the game's thread ---

/// Hand `bytes` to the host, post `request`, and wait for the reply. The worker
/// blocks here; the host's own thread does not.
pub fn suspend(slot: u64, request: Request, bytes: &[u8]) -> Result<Reply, u32> {
    let channel = channel();
    let mut sessions = locked(channel.sessions.lock())?;
    sessions.try_reserve(1).map_err(|_| 6u32)?;
    {
        let shared = sessions.entry(slot).or_default();
        shared.output.try_reserve(bytes.len()).map_err(|_| 6u32)?;
        shared.output.extend_from_slice(bytes);
        shared.request = Some(request);
    }
    channel.changed.notify_all();
    loop {
        {
            let shared = sessions.entry(slot).or_default();
            if let Some(reply) = shared.reply.take() {
                return Ok(reply);
            }
            if shared.closed {
                shared.request = None;
                return Ok(Reply::Line(None));
            }
        }
        sessions = locked(channel.changed.wait(sessions))?;
    }
}

/// Hand output to the host without asking for anything, as `flushOutput` does.
pub fn publish(slot: u64, bytes: &[u8]) -> Result<(), u32> {
    if bytes.is_empty() {
        return Ok(());
    }
    with(slot, |shared| {
        shared.output.try_reserve(bytes.len()).map_err(|_| 6u32)?;
        shared.output.extend_from_slice(bytes);
        Ok(())
    })
}

/// Offer the results of an inspection query for the host to read.
pub fn publish_results(slot: u64, values: &[u64]) -> Result<(), u32> {
    with(slot, |shared| {
        shared.results = Vec::new();
        shared.results.try_reserve(values.len()).map_err(|_| 6u32)?;
        shared.results.extend_from_slice(values);
        Ok(())
    })
}

/// Take the inspection the host asked for, if any. The worker calls this
/// between cycles, where there is no live cycle to read across.
pub fn take_inspection(slot: u64) -> Option<(u32, u64, u64)> {
    let mut sessions = channel().sessions.lock().ok()?;
    sessions.get_mut(&slot)?.inspection.take()
}

/// Report the session's outcome. The consumer calls this on the worker thread
/// once the game entry has returned, which is what ends the host's loop.
pub fn finish(slot: u64, outcome: u64) -> Result<(), u32> {
    with(slot, |shared| {
        shared.request = None;
        shared.finished = Some(outcome);
        Ok(())
    })
}

// --- the host side: called from the consumer's own thread ---

/// Wait for the game to suspend or finish. Returns the request code, or 2 once
/// the session has ended. This is the only call that blocks the host, and it
/// blocks on the game's progress rather than on a console.
pub fn poll(slot: u64) -> Result<u32, u32> {
    let channel = channel();
    let mut sessions = locked(channel.sessions.lock())?;
    loop {
        {
            sessions.try_reserve(1).map_err(|_| 6u32)?;
            let shared = sessions.entry(slot).or_default();
            if let Some(request) = shared.request {
                return Ok(request.code());
            }
            if shared.finished.is_some() {
                return Ok(2);
            }
        }
        sessions = locked(channel.changed.wait(sessions))?;
    }
}

fn read<T>(slot: u64, act: impl FnOnce(&Shared) -> T, absent: T) -> T {
    let Ok(sessions) = channel().sessions.lock() else {
        return absent;
    };
    sessions.get(&slot).map_or(absent, act)
}

/// One byte of the output handed over so far, or 256 past the end.
pub fn output_byte(slot: u64, offset: u64) -> u32 {
    read(
        slot,
        |shared| {
            usize::try_from(offset)
                .ok()
                .and_then(|index| shared.output.get(index))
                .map_or(256, |byte| u32::from(*byte))
        },
        256,
    )
}

/// How many bytes of output are waiting, so a host can size one read instead of
/// asking a byte at a time.
pub fn output_len(slot: u64) -> u64 {
    read(slot, |shared| shared.output.len() as u64, 0)
}

/// Eight bytes of output at once, little-endian, zero-filled past the end. The
/// boundary stays pointer-free; it just stops costing a call per byte.
pub fn output_word(slot: u64, offset: u64) -> u64 {
    read(
        slot,
        |shared| {
            let Ok(start) = usize::try_from(offset) else {
                return 0;
            };
            let mut word = 0u64;
            for step in 0..8 {
                if let Some(byte) = shared.output.get(start + step) {
                    word |= u64::from(*byte) << (8 * step);
                }
            }
            word
        },
        0,
    )
}

/// Forget the output the host has printed, so the next handover starts at zero.
///
/// Events go with it. They describe the same turn as the text does, so a host
/// that has taken one has taken both; leaving them would mean replaying an
/// event whose sentence has already been printed.
pub fn drained(slot: u64) -> Result<(), u32> {
    with(slot, |shared| {
        shared.output = Vec::new();
        shared.events = Vec::new();
        shared.event_values = None;
        Ok(())
    })
}

/// Start an event: an id, and no entities yet.
///
/// The record is complete as soon as it is written, so a host draining
/// mid-event still reads something well-formed. `event_subject` extends the
/// last record rather than opening a new one.
pub fn event(slot: u64, id: &[u8]) -> Result<(), u32> {
    with(slot, |shared| {
        if id.is_empty() || id.len() > PACKET_LIMIT {
            return Err(5);
        }
        let padded = id.len().div_ceil(8) * 8;
        let words = 3 + padded / 8;
        if shared.events.len() + words * 8 > PACKET_LIMIT {
            return Err(5);
        }
        shared.event_values = None;
        shared.events.try_reserve(words * 8).map_err(|_| 6u32)?;
        shared
            .events
            .extend_from_slice(&(words as u64).to_le_bytes());
        shared
            .events
            .extend_from_slice(&(id.len() as u64).to_le_bytes());
        shared.events.extend_from_slice(id);
        shared
            .events
            .extend(std::iter::repeat_n(0u8, padded - id.len()));
        // No entities yet.
        shared.events.extend_from_slice(&0u64.to_le_bytes());
        Ok(())
    })
}

/// Undo or rollback invalidates previously observed movement. Discard any
/// undrained provisional events and require a fresh snapshot at the boundary.
pub fn resync(slot: u64) -> Result<(), u32> {
    with(slot, |shared| {
        shared.events.clear();
        shared.event_values = None;
        shared.snapshot = 0;
        Ok(())
    })?;
    event(slot, b"world.resync")
}

/// A restored world invalidates queued inspection results and prior output.
pub(crate) fn restored(slot: u64) -> Result<(), u32> {
    with(slot, |s| {
        s.results.clear();
        s.inspection = None;
        s.output.clear();
        s.pending_subjects.clear();
        s.pending_size = 0;
        s.pending_text.clear();
        s.pending_verb = None;
        Ok(())
    })?;
    resync(slot)
}

/// Name an entity the last event concerns.
///
/// Refused when no event has been started: a subject with nothing to belong to
/// would corrupt the stream rather than merely be unhelpful.
pub fn event_subject(slot: u64, handle: u64) -> Result<(), u32> {
    with(slot, |shared| {
        let start = last_event(&shared.events).ok_or(11u32)?;
        if shared.event_values.is_some() {
            return Err(11);
        }
        if shared.events.len() + 8 > PACKET_LIMIT {
            return Err(5);
        }
        shared.events.try_reserve(8).map_err(|_| 6u32)?;
        // words, id length, the padded id, then the count.
        let count_at =
            start + 16 + 8 * usize::try_from(id_words(&shared.events, start)).map_err(|_| 11u32)?;
        bump(&mut shared.events, count_at);
        bump(&mut shared.events, start);
        shared.events.extend_from_slice(&handle.to_le_bytes());
        Ok(())
    })
}

/// Read one little-endian word out of the event stream.
pub fn event_value(slot: u64, tag: u64, bytes: &[u8]) -> Result<(), u32> {
    let valid = match tag {
        0 | 1 => bytes.is_empty(),
        2 | 3 => bytes.len() == 8,
        4 => std::str::from_utf8(bytes).is_ok(),
        _ => false,
    };
    if !valid {
        return Err(16);
    }
    if bytes.len() > PACKET_LIMIT {
        return Err(5);
    }
    with(slot, |s| {
        let start = last_event(&s.events).ok_or(11u32)?;
        let padded = bytes.len().div_ceil(8) * 8;
        let extra = 16 + padded + if s.event_values.is_none() { 8 } else { 0 };
        if s.events.len() + extra > PACKET_LIMIT {
            return Err(5);
        }
        s.events.try_reserve(extra).map_err(|_| 6u32)?;
        let count = match s.event_values {
            Some(at) => at,
            None => {
                let at = s.events.len();
                s.event_values = Some(at);
                s.events.extend_from_slice(&0u64.to_le_bytes());
                at
            }
        };
        bump(&mut s.events, count);
        s.events.extend_from_slice(&tag.to_le_bytes());
        s.events
            .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        s.events.extend_from_slice(bytes);
        s.events
            .extend(std::iter::repeat_n(0, padded - bytes.len()));
        let words = ((s.events.len() - start) / 8) as u64;
        s.events[start..start + 8].copy_from_slice(&words.to_le_bytes());
        Ok(())
    })
}

/// Issue a token, or atomically check and consume one. Never rewound by history.
pub fn snapshot_token(slot: u64, token: u32) -> Result<u32, u32> {
    with(slot, |s| {
        if token == 0 {
            let next = next_snapshot(&NEXT_SNAPSHOT)?;
            s.snapshot = next;
            Ok(next)
        } else if s.snapshot == token {
            s.snapshot = 0;
            Ok(1)
        } else {
            Ok(0)
        }
    })
}

/// Append a copied action argument. Text consumes bytes assembled by reply_byte.
pub fn reply_value(slot: u64, tag: u32, payload: u64) -> Result<(), u32> {
    with(slot, |s| {
        let size = 16 + if tag == 4 { s.pending_text.len() } else { 0 };
        if s.pending_size + s.pending_text.len() + 16 > PACKET_LIMIT {
            return Err(5);
        }
        s.pending_subjects.try_reserve(1).map_err(|_| 6u32)?;
        let value = match tag {
            0 if payload == 0 => Scalar::Nil,
            1 if payload == 0 => Scalar::True,
            2 => Scalar::Integer(i32::try_from(payload as i64).map_err(|_| 18u32)?),
            3 => Scalar::Entity(payload),
            4 if payload == 0 => {
                std::str::from_utf8(&s.pending_text).map_err(|_| 17u32)?;
                Scalar::Text(std::mem::take(&mut s.pending_text))
            }
            _ => return Err(16),
        };
        s.pending_subjects.push(value);
        s.pending_size += size;
        Ok(())
    })
}

fn read_word(events: &[u8], at: usize) -> u64 {
    let mut word = 0u64;
    for step in 0..8 {
        if let Some(byte) = events.get(at + step) {
            word |= u64::from(*byte) << (8 * step);
        }
    }
    word
}

/// Add one to the word at this offset: a record's length, or its entity count.
fn bump(events: &mut [u8], at: usize) {
    let raised = read_word(events, at).saturating_add(1);
    if at + 8 <= events.len() {
        events[at..at + 8].copy_from_slice(&raised.to_le_bytes());
    }
}

/// How many words the id of the record starting here occupies, padding included.
fn id_words(events: &[u8], start: usize) -> u64 {
    read_word(events, start + 8).div_ceil(8)
}

/// Where the last record starts, by walking from the front. A turn writes a
/// handful of events, so walking is cheaper than carrying an index that could
/// disagree with the stream.
fn last_event(events: &[u8]) -> Option<usize> {
    if events.is_empty() {
        return None;
    }
    let mut at = 0usize;
    let mut previous = None;
    while at + 8 <= events.len() {
        let words = read_word(events, at);
        if words == 0 {
            return previous;
        }
        previous = Some(at);
        at += usize::try_from(words).ok()? * 8;
    }
    previous
}

/// How many bytes of event stream are waiting, and eight of them at a time —
/// the same shape as the output stream, for the same reason.
pub fn event_len(slot: u64) -> u64 {
    read(slot, |shared| shared.events.len() as u64, 0)
}

pub fn event_word(slot: u64, offset: u64) -> u64 {
    read(
        slot,
        |shared| {
            let Ok(start) = usize::try_from(offset) else {
                return 0;
            };
            read_word(&shared.events, start)
        },
        0,
    )
}

/// How many results the last inspection query produced, and one of them.
pub fn result_len(slot: u64) -> u64 {
    read(slot, |shared| shared.results.len() as u64, 0)
}

pub fn result(slot: u64, index: u64) -> u64 {
    read(
        slot,
        |shared| {
            usize::try_from(index)
                .ok()
                .and_then(|index| shared.results.get(index))
                .copied()
                .unwrap_or(0)
        },
        0,
    )
}

/// Ask for a bounded, read-only look at stored state. The answer arrives at the
/// game's next safe boundary, so an inspector sees the world between commands
/// rather than part-way through one.
/// Ask for one bounded, read-only inspection, answered at the game's next safe
/// boundary.
///
/// **A second request over an unanswered one is refused**. It used
/// to replace it silently, so a host that queued several got the last one and
/// no way to know the rest were gone — which is a host binding the wrong
/// handle to a name and never finding out. One turn answers one inspection;
/// asking for more than that is a mistake in the host, and it now says so.
pub fn inspect(slot: u64, kind: u32, a: u64, b: u64) -> Result<(), u32> {
    with(slot, |shared| {
        if shared.inspection.is_some() {
            return Err(10);
        }
        shared.inspection = Some((kind, a, b));
        Ok(())
    })
}

/// Bounded byte transport for logical saves, serviced only at command input.
/// 0 clear upload; 1 append byte; 2 save; 3 restore; 4 status; 5 length; 6 byte.
pub fn persistence(slot: u64, op: u32, value: u64) -> Result<u64, u32> {
    with(slot, |s| {
        if op == 4 {
            return Ok(if s.save_busy { 10 } else { s.save_status });
        }
        if s.save_busy {
            return Err(10);
        }
        match op {
            0 => {
                s.save_bytes.clear();
                s.save_status = 0;
                Ok(0)
            }
            1 => {
                let byte = u8::try_from(value).map_err(|_| 11u32)?;
                if s.save_bytes.len() >= crate::save::MAX_BYTES {
                    return Err(5);
                }
                s.save_bytes.try_reserve(1).map_err(|_| 6u32)?;
                s.save_bytes.push(byte);
                Ok(0)
            }
            2 | 3 => {
                if s.request != Some(Request::Command)
                    || s.reply.is_some()
                    || !s.pending_text.is_empty()
                    || s.pending_verb.is_some()
                    || !s.pending_subjects.is_empty()
                {
                    return Err(10);
                }
                let bytes = std::mem::take(&mut s.save_bytes);
                s.save_busy = true;
                s.request = None;
                s.reply = Some(Reply::Persistence {
                    restore: op == 3,
                    bytes,
                });
                Ok(0)
            }
            5 => Ok(s.save_bytes.len() as u64),
            6 => Ok(usize::try_from(value)
                .ok()
                .and_then(|i| s.save_bytes.get(i))
                .map_or(256, |b| u64::from(*b))),
            _ => Err(11),
        }
    })
}
pub(crate) fn saved(slot: u64, result: Result<Vec<u8>, u32>) -> Result<(), u32> {
    with(slot, |s| {
        match result {
            Ok(bytes) => {
                s.save_bytes = bytes;
                s.save_status = 0;
            }
            Err(code) => {
                s.save_bytes.clear();
                s.save_status = u64::from(code);
            }
        }
        s.save_busy = false;
        Ok(())
    })
}

/// Add one byte to the text reply being assembled.
pub fn reply_byte(slot: u64, byte: u8) -> Result<(), u32> {
    with(slot, |shared| {
        if shared.pending_size + shared.pending_text.len() >= PACKET_LIMIT {
            return Err(5);
        }
        shared.pending_text.try_reserve(1).map_err(|_| 6u32)?;
        shared.pending_text.push(byte);
        Ok(())
    })
}

/// Choose an action rather than a line: the host decided what to do without a
/// parser, which is the CRPG shape of an intent.
pub fn reply_action(slot: u64, verb: u32) -> Result<(), u32> {
    with(slot, |shared| {
        shared.pending_verb = Some(verb);
        Ok(())
    })
}

/// Name one entity the pending action applies to.
pub fn reply_subject(slot: u64, handle: u64) -> Result<(), u32> {
    with(slot, |shared| {
        if shared.pending_size + shared.pending_text.len() + 16 > PACKET_LIMIT {
            return Err(5);
        }
        shared.pending_subjects.try_reserve(1).map_err(|_| 6u32)?;
        shared.pending_subjects.push(Scalar::Entity(handle));
        shared.pending_size += 16;
        Ok(())
    })
}

/// Deliver the assembled reply and let the game run on.
pub fn resume(slot: u64) -> Result<(), u32> {
    with(slot, |shared| {
        let reply = match (shared.request, shared.pending_verb.take()) {
            (Some(Request::HostAction), _) => {
                Reply::Accepted(shared.pending_text.first() != Some(&b'0'))
            }
            (Some(Request::EngineQuery), _) => {
                let text = std::str::from_utf8(&shared.pending_text).unwrap_or("0");
                Reply::Value(text.trim().parse::<i64>().unwrap_or(0))
            }
            (_, Some(verb)) => Reply::Action {
                verb,
                subjects: std::mem::take(&mut shared.pending_subjects),
            },
            (_, None) => {
                let bytes = std::mem::take(&mut shared.pending_text);
                Reply::Line(Some(String::from_utf8(bytes).map_err(|_| 17u32)?))
            }
        };
        shared.pending_text = Vec::new();
        shared.pending_subjects = Vec::new();
        shared.pending_size = 0;
        shared.reply = Some(reply);
        shared.request = None;
        Ok(())
    })
}

/// Report that no more input will come. The pending request, and every request
/// after it, reads as the end of input.
pub fn close(slot: u64) -> Result<(), u32> {
    with(slot, |shared| {
        shared.closed = true;
        shared.reply = Some(Reply::Line(None));
        shared.request = None;
        Ok(())
    })
}

/// The finished session's outcome, or 0 while it is still running.
pub fn outcome(slot: u64) -> u64 {
    read(slot, |shared| shared.finished.unwrap_or(0), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_services_requests_while_the_worker_waits_for_them() {
        let slot = 11;
        reset(slot).unwrap();
        let worker = std::thread::spawn(move || {
            let first = suspend(slot, Request::Line, b"> ").unwrap();
            let second = suspend(slot, Request::Line, b"ok ").unwrap();
            finish(slot, 7).unwrap();
            (first, second)
        });
        // The host sees the complete prompt before supplying an answer.
        assert_eq!(poll(slot).unwrap(), 0);
        assert_eq!(output_byte(slot, 0), u32::from(b'>'));
        assert_eq!(output_len(slot), 2);
        // "> " little-endian: '>' then ' '.
        assert_eq!(output_word(slot, 0) & 0xffff, 0x2000 | u64::from(b'>'));
        drained(slot).unwrap();
        for byte in b"look" {
            reply_byte(slot, *byte).unwrap();
        }
        resume(slot).unwrap();
        assert_eq!(poll(slot).unwrap(), 0);
        close(slot).unwrap();
        assert_eq!(poll(slot).unwrap(), 2);
        assert_eq!(outcome(slot), 7);
        let (first, second) = worker.join().unwrap();
        assert!(matches!(first, Reply::Line(Some(ref text)) if text == "look"));
        assert!(matches!(second, Reply::Line(None)));
    }

    #[test]
    fn a_host_may_submit_an_action_instead_of_a_line() {
        // a CRPG chooses the action; a parser front end is one way to
        // arrive at the same thing.
        let slot = 12;
        reset(slot).unwrap();
        let worker = std::thread::spawn(move || suspend(slot, Request::Line, b"").unwrap());
        assert_eq!(poll(slot).unwrap(), 0);
        reply_action(slot, 5).unwrap();
        reply_subject(slot, 99).unwrap();
        reply_subject(slot, 100).unwrap();
        resume(slot).unwrap();
        let reply = worker.join().unwrap();
        match reply {
            Reply::Action { verb, subjects } => {
                assert_eq!(verb, 5);
                assert_eq!(subjects, vec![Scalar::Entity(99), Scalar::Entity(100)]);
            }
            other => panic!("expected an action, got {other:?}"),
        }
        discard(slot).unwrap();
    }

    #[test]
    fn two_sessions_do_not_service_each_others_requests() {
        // the handoff is keyed, so one process can run several.
        let (a, b) = (21, 22);
        reset(a).unwrap();
        reset(b).unwrap();
        let first = std::thread::spawn(move || suspend(a, Request::Line, b"a").unwrap());
        assert_eq!(poll(a).unwrap(), 0);
        for byte in b"one" {
            reply_byte(a, *byte).unwrap();
        }
        resume(a).unwrap();
        assert!(matches!(first.join().unwrap(), Reply::Line(Some(ref t)) if t == "one"));
        // The other session never saw any of that.
        assert_eq!(output_len(b), 0);
        assert_eq!(outcome(b), 0);
        discard(a).unwrap();
        discard(b).unwrap();
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;

    /// Read the event stream back the way a host does: walk the records.
    fn records(slot: u64) -> Vec<(String, Vec<u64>)> {
        let bytes: Vec<u8> = (0..event_len(slot))
            .step_by(8)
            .flat_map(|offset| event_word(slot, offset).to_le_bytes())
            .collect();
        let mut found = Vec::new();
        let mut at = 0usize;
        while at + 8 <= bytes.len() {
            let words = read_word(&bytes, at) as usize;
            if words == 0 {
                break;
            }
            let id_len = read_word(&bytes, at + 8) as usize;
            let id = String::from_utf8(bytes[at + 16..at + 16 + id_len].to_vec()).unwrap();
            let padded = id_len.div_ceil(8) * 8;
            let count = read_word(&bytes, at + 16 + padded) as usize;
            let mut handles = Vec::new();
            for step in 0..count {
                handles.push(read_word(&bytes, at + 24 + padded + step * 8));
            }
            found.push((id, handles));
            at += words * 8;
        }
        found
    }

    /// an event is an id and the entities it concerns, and it is
    /// self-describing so no table has to be agreed in advance.
    #[test]
    fn events_carry_an_id_and_its_entities() {
        let slot = 7100;
        reset(slot).unwrap();
        event(slot, b"take.ok").unwrap();
        event_subject(slot, 0x1234_5678).unwrap();
        event(slot, b"putin.ok").unwrap();
        event_subject(slot, 11).unwrap();
        event_subject(slot, 22).unwrap();
        event(slot, b"room.dark").unwrap();
        assert_eq!(
            records(slot),
            vec![
                ("take.ok".to_owned(), vec![0x1234_5678]),
                ("putin.ok".to_owned(), vec![11, 22]),
                ("room.dark".to_owned(), vec![]),
            ]
        );
        discard(slot).unwrap();
    }

    /// An id whose length is a multiple of eight has no padding, which is the
    /// case an off-by-one in the walk would survive everywhere else.
    #[test]
    fn an_id_that_fills_its_words_exactly_still_reads_back() {
        let slot = 7101;
        reset(slot).unwrap();
        event(slot, b"exactly8").unwrap();
        event_subject(slot, 99).unwrap();
        event(slot, b"a").unwrap();
        assert_eq!(
            records(slot),
            vec![("exactly8".to_owned(), vec![99]), ("a".to_owned(), vec![]),]
        );
        discard(slot).unwrap();
    }

    /// Draining takes the events with the text. They describe the same turn, so
    /// a host that has taken one has taken both.
    #[test]
    fn draining_the_output_takes_the_events_with_it() {
        let slot = 7102;
        reset(slot).unwrap();
        event(slot, b"take.ok").unwrap();
        assert!(event_len(slot) > 0);
        drained(slot).unwrap();
        assert_eq!(event_len(slot), 0);
        assert!(records(slot).is_empty());
        discard(slot).unwrap();
    }

    #[test]
    fn resync_replaces_undrained_facts_and_also_survives_an_earlier_drain() {
        let slot = 7104;
        reset(slot).unwrap();
        event(slot, b"actor.move").unwrap();
        event_subject(slot, 123).unwrap();
        resync(slot).unwrap();
        assert_eq!(records(slot), vec![("world.resync".to_owned(), vec![])]);
        drained(slot).unwrap();
        resync(slot).unwrap();
        assert_eq!(records(slot), vec![("world.resync".to_owned(), vec![])]);
        discard(slot).unwrap();
    }

    /// A subject with no event to belong to would corrupt the stream, so it is
    /// refused rather than written somewhere plausible.
    #[test]
    fn a_subject_without_an_event_is_refused() {
        let slot = 7103;
        reset(slot).unwrap();
        assert_eq!(event_subject(slot, 5), Err(11));
        assert_eq!(event_len(slot), 0);
        discard(slot).unwrap();
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    #[test]
    fn only_command_boundaries_service_saves_and_the_transfer_is_bounded() {
        let slot = 998_321;
        reset(slot).unwrap();
        let worker = std::thread::spawn(move || suspend(slot, Request::Line, b"prompt").unwrap());
        assert_eq!(poll(slot), Ok(0));
        assert_eq!(persistence(slot, 2, 0), Err(10));
        close(slot).unwrap();
        worker.join().unwrap();
        reset(slot).unwrap();
        let worker = std::thread::spawn(move || {
            let reply = suspend(slot, Request::Command, b"").unwrap();
            assert!(matches!(reply, Reply::Persistence { restore: false, .. }));
            saved(slot, Ok(vec![1, 2, 3])).unwrap();
            suspend(slot, Request::Command, b"").unwrap();
        });
        assert_eq!(poll(slot), Ok(0));
        assert_eq!(persistence(slot, 1, 256), Err(11));
        assert_eq!(persistence(slot, 2, 0), Ok(0));
        assert_eq!(poll(slot), Ok(0));
        assert_eq!(persistence(slot, 4, 0), Ok(0));
        assert_eq!(persistence(slot, 5, 0), Ok(3));
        assert_eq!(persistence(slot, 6, 2), Ok(3));
        assert_eq!(persistence(slot, 6, 3), Ok(256));
        close(slot).unwrap();
        worker.join().unwrap();
        discard(slot).unwrap();
    }
}

#[cfg(test)]
mod scalar_transport_tests {
    use super::*;

    #[test]
    fn values_preserve_legacy_prefix_and_are_length_delimited() {
        let slot = 90401;
        reset(slot).unwrap();
        event(slot, b"example").unwrap();
        event_subject(slot, 44).unwrap();
        event_value(slot, 4, "a\n\"λ".as_bytes()).unwrap();
        event_value(slot, 2, &(-17i64).to_le_bytes()).unwrap();
        event_value(slot, 0, &[]).unwrap();
        assert_eq!(event_word(slot, 0) * 8, event_len(slot));
        assert_eq!(event_word(slot, 24), 1); // legacy subject count
        assert_eq!(event_word(slot, 32), 44);
        assert_eq!(event_word(slot, 40), 3); // typed value count
        assert_eq!(event_word(slot, 48), 4);
        assert_eq!(event_word(slot, 56), 5); // UTF-8 byte count, not character count
        assert_eq!(event_word(slot, 88) as i64, -17);
        assert_eq!(event_subject(slot, 45), Err(11));
        event(slot, b"next").unwrap();
        event_subject(slot, 46).unwrap();
        discard(slot).unwrap();
    }

    #[test]
    fn oversize_and_invalid_values_do_not_publish_partial_fields() {
        let slot = 90402;
        reset(slot).unwrap();
        event(slot, b"x").unwrap();
        let before = event_len(slot);
        assert_eq!(event_value(slot, 4, &[255]), Err(16));
        assert_eq!(event_value(slot, 4, &vec![b'a'; PACKET_LIMIT]), Err(5));
        assert_eq!(event_len(slot), before);
        event_value(slot, 4, &vec![b'a'; PACKET_LIMIT - 56]).unwrap();
        assert_eq!(event_len(slot), PACKET_LIMIT as u64);
        assert_eq!(event(slot, b"overflow"), Err(5));
        drained(slot).unwrap();
        event(slot, b"reusable").unwrap();
        discard(slot).unwrap();
    }

    #[test]
    fn snapshots_are_single_use_isolated_and_not_rewound() {
        let a = 90403;
        let b = 90404;
        reset(a).unwrap();
        reset(b).unwrap();
        let first = snapshot_token(a, 0).unwrap();
        assert_eq!(snapshot_token(b, first), Ok(0));
        assert_eq!(snapshot_token(a, first), Ok(1));
        assert_eq!(snapshot_token(a, first), Ok(0));
        let second = snapshot_token(a, 0).unwrap();
        resync(a).unwrap();
        assert_eq!(snapshot_token(a, second), Ok(0));
        let third = snapshot_token(a, 0).unwrap();
        reset(a).unwrap();
        assert_eq!(snapshot_token(a, third), Ok(0));
        assert!(snapshot_token(a, 0).unwrap() > third);
        let exhausted = AtomicU32::new(i32::MAX as u32);
        assert_eq!(next_snapshot(&exhausted), Err(18));
        assert_eq!(exhausted.load(Ordering::Relaxed), i32::MAX as u32);
        discard(a).unwrap();
        discard(b).unwrap();
    }

    #[test]
    fn action_values_are_copied_checked_and_bounded() {
        let slot = 90405;
        reset(slot).unwrap();
        reply_action(slot, 81).unwrap();
        reply_value(slot, 2, (-123i64) as u64).unwrap();
        for b in "choice-λ".bytes() {
            reply_byte(slot, b).unwrap();
        }
        reply_value(slot, 4, 0).unwrap();
        reply_subject(slot, 99).unwrap();
        with(slot, |s| {
            assert_eq!(
                s.pending_subjects,
                vec![
                    Scalar::Integer(-123),
                    Scalar::Text("choice-λ".as_bytes().to_vec()),
                    Scalar::Entity(99)
                ]
            );
            assert!(s.pending_text.is_empty());
            Ok(())
        })
        .unwrap();
        assert_eq!(reply_value(slot, 2, i64::MAX as u64), Err(18));
        for _ in 0..PACKET_LIMIT {
            if reply_byte(slot, b'x').is_err() {
                break;
            }
        }
        assert_eq!(reply_byte(slot, b'x'), Err(5));
        discard(slot).unwrap();
    }
}
