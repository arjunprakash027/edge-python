use compiler::modules::lexer::{lex, TokenType};
use compiler::modules::lexer::tables::token_to_str;
use compiler::modules::parser::{Parser, SSAChunk};
use compiler::modules::vm::{VM, Val};
use std::{panic, sync::mpsc, thread, time::{Duration, Instant, SystemTime}};

const MAX_LEN: usize = 2048;
const SAVE_DIR: &str = "crashes";
const PRINT_INTERVAL: u64 = 10_000;
const MAX_SECS: u64 = 600; // 10 minutes
const VM_TIMEOUT: Duration = Duration::from_millis(200);
const SLOW_THRESHOLD: Duration = Duration::from_millis(50);

struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0x517cc1b727220a95, |d| d.as_nanos() as u64);
        Self(if seed == 0 { 1 } else { seed })
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn usize_in(&mut self, n: usize) -> usize { if n == 0 { 0 } else { (self.next() as usize) % n } }
    fn byte(&mut self) -> u8 { self.next() as u8 }
}

// Seeds loaded from the existing test corpus at compile time
fn load_seeds() -> Vec<String> {
    const JSON: &str = include_str!("../../tests/cases/vm.json");
    let mut seeds = Vec::new();
    for line in JSON.lines() {
        let Some(start) = line.find("\"src\": \"") else { continue };
        let rest = &line[start + 8..];
        let mut src = String::new();
        let mut chars = rest.chars();
        loop {
            match chars.next() {
                Some('\\') => match chars.next() {
                    Some('n')  => src.push('\n'),
                    Some('t')  => src.push('\t'),
                    Some('"')  => src.push('"'),
                    Some('\\') => src.push('\\'),
                    _          => {}
                },
                Some('"') | None => break,
                Some(c) => src.push(c),
            }
        }
        if !src.is_empty() { seeds.push(src); }
    }
    seeds
}

// Keyword variants sourced directly from the lexer enum
const KEYWORD_TOKENS: &[TokenType] = &[
    TokenType::If, TokenType::Else, TokenType::Elif, TokenType::For,
    TokenType::While, TokenType::Def, TokenType::Class, TokenType::Return,
    TokenType::Import,TokenType::From, TokenType::Try, TokenType::Except,
    TokenType::With, TokenType::Yield, TokenType::Async, TokenType::Await,
    TokenType::Pass, TokenType::Break, TokenType::Continue,
    TokenType::True, TokenType::False, TokenType::None,
    TokenType::And, TokenType::Or, TokenType::Not,
    TokenType::In, TokenType::Is, TokenType::Lambda,
    TokenType::Assert, TokenType::Del, TokenType::Raise,
];

// token_to_str returns "'keyword'" — strip the surrounding single quotes
fn rand_keyword(rng: &mut Rng) -> &'static str {
    let s = token_to_str(&KEYWORD_TOKENS[rng.usize_in(KEYWORD_TOKENS.len())]);
    &s[1..s.len() - 1]
}

const NAMES: &[&str] = &["x", "y", "z", "n", "i", "j", "a", "b", "f", "g", "result", "val"];

fn rand_name(rng: &mut Rng) -> &'static str { NAMES[rng.usize_in(NAMES.len())] }

// Boundary i64 values derived from Val's NaN-box limits
const BOUNDARIES: [i64; 13] = [
    0, 1, -1,
    Val::INT_MAX, Val::INT_MIN,
    Val::INT_MAX - 1, Val::INT_MIN + 1,
    i32::MAX as i64, i32::MIN as i64,
    i16::MAX as i64, i8::MIN as i64,
    255, 65535,
];

fn boundary_int(rng: &mut Rng) -> i64 { BOUNDARIES[rng.usize_in(BOUNDARIES.len())] }

fn rand_int(rng: &mut Rng) -> String {
    if rng.usize_in(4) == 0 { boundary_int(rng).to_string() }
    else { (rng.next() as i64).to_string() }
}

fn mutate(src: &str, corpus: &[String], rng: &mut Rng) -> String {
    match rng.usize_in(10) {
        0 => byte_flip(src, rng),
        1 => insert_keyword(src, rng),
        2 => drop_line(src, rng),
        3 => duplicate_line(src, rng),
        4 => splice(src, corpus, rng),
        5 => inject_boundary(src, rng),
        6 => deep_nest(rng),
        7 => token_shuffle(src, rng),
        8 => indent_bomb(rng),
        _ => add_comment(src, rng),
    }
}

// XOR a random byte: surfaces malformed UTF-8 and broken literals
fn byte_flip(src: &str, rng: &mut Rng) -> String {
    let mut bytes = src.as_bytes().to_vec();
    if bytes.is_empty() { return src.to_string(); }
    let i = rng.usize_in(bytes.len());
    bytes[i] ^= rng.byte() | 1;
    String::from_utf8_lossy(&bytes).into_owned()
}

fn with_lines(src: &str, f: impl FnOnce(&mut Vec<&str>)) -> String {
    let mut lines: Vec<&str> = src.lines().collect();
    f(&mut lines);
    lines.join("\n")
}

fn insert_keyword(src: &str, rng: &mut Rng) -> String {
    let kw = rand_keyword(rng);
    let name = rand_name(rng);
    let n = rand_int(rng);
    let snippet = match rng.usize_in(4) {
        0 => format!("{name} = {n}"),
        1 => format!("{kw} {name}:"),
        2 => format!("print({kw})"),
        _ => kw.to_string(),
    };
    let mut lines: Vec<&str> = src.lines().collect();
    let idx = rng.usize_in(lines.len() + 1);
    lines.insert(idx, &snippet);
    lines.join("\n")
}

// Dropping a line produces undefined name references and broken control flow
fn drop_line(src: &str, rng: &mut Rng) -> String {
    if src.lines().count() <= 1 { return src.to_string(); }
    with_lines(src, |lines| { lines.remove(rng.usize_in(lines.len())); })
}

// Duplicating a line generates double assignments and extra phi nodes
fn duplicate_line(src: &str, rng: &mut Rng) -> String {
    if src.is_empty() { return src.to_string(); }
    with_lines(src, |lines| { let idx = rng.usize_in(lines.len()); lines.insert(idx, lines[idx]); })
}

fn splice(src: &str, corpus: &[String], rng: &mut Rng) -> String {
    if corpus.is_empty() { return src.to_string(); }
    let other = &corpus[rng.usize_in(corpus.len())];
    let a: Vec<&str> = src.lines().collect();
    let b: Vec<&str> = other.lines().collect();
    let ca = rng.usize_in(a.len().max(1));
    let cb = rng.usize_in(b.len().max(1));
    let mut out = a[..ca].to_vec();
    out.extend_from_slice(&b[cb..]);
    out.join("\n")
}

fn inject_boundary(src: &str, rng: &mut Rng) -> String {
    let boundary = boundary_int(rng).to_string();
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 24);
    let mut i = 0;
    let mut done = false;
    while i < bytes.len() {
        if !done && bytes[i].is_ascii_digit() {
            out.push_str(&boundary);
            while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
            done = true;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

// Targets MAX_EXPR_DEPTH = 200 in the parser
fn deep_nest(rng: &mut Rng) -> String {
    let depth = 100 + rng.usize_in(120);
    let name = rand_name(rng);
    let n = rand_int(rng);
    let (o, c) = match rng.usize_in(3) {
        0 => ("(", ")"),
        1 => ("[", "]"),
        _ => return format!("x = {}\n{}", n, "if x:\n    pass\n".repeat(depth)),
    };
    format!("{name} = {}{n}{}\nprint({name})", o.repeat(depth), c.repeat(depth))
}

// Swapping tokens exercises parser error recovery on invalid sequences
fn token_shuffle(src: &str, rng: &mut Rng) -> String {
    let (tokens, _) = lex(src);
    if tokens.len() < 2 { return src.to_string(); }
    let i = rng.usize_in(tokens.len());
    let j = rng.usize_in(tokens.len());
    let mut parts: Vec<&str> = tokens.iter()
        .map(|t| if t.start < t.end && t.end <= src.len() { &src[t.start..t.end] } else { "" })
        .collect();
    parts.swap(i, j);
    parts.join(" ")
}

// Stresses lexer indentation tracking via MAX_INDENT_DEPTH = 100
fn indent_bomb(rng: &mut Rng) -> String {
    let depth = 50 + rng.usize_in(60);
    let mut out = String::with_capacity(depth * 14);
    for _ in 0..depth { out.push_str("if True:\n"); }
    out.push_str(&"    ".repeat(depth));
    out.push_str("pass");
    out
}

fn add_comment(src: &str, rng: &mut Rng) -> String {
    let comment = format!("# {}", rand_int(rng));
    let mut lines: Vec<&str> = src.lines().collect();
    let idx = rng.usize_in(lines.len().max(1));
    lines.insert(idx, &comment);
    lines.join("\n")
}

// Bit N set when opcode discriminant N appears in chunk, its functions, or classes
fn opcode_bitmap(chunk: &SSAChunk) -> u128 {
    let mut bits = 0u128;
    for instr in &chunk.instructions {
        bits |= 1u128 << ((instr.opcode as u8) & 127);
    }
    for (_, sub, _, _) in &chunk.functions {
        bits |= opcode_bitmap(sub);
    }
    for sub in &chunk.classes {
        bits |= opcode_bitmap(sub);
    }
    bits
}

// Per-phase timing collected across all runs
struct Perf {
    lex:   (Duration, Duration), // (sum, max)
    parse: (Duration, Duration),
    vm:    (Duration, Duration),
    count: u64,
}

impl Perf {
    fn new() -> Self {
        let z = (Duration::ZERO, Duration::ZERO);
        Self { lex: z, parse: z, vm: z, count: 0 }
    }
    fn record(&mut self, t_lex: Duration, t_parse: Duration, t_vm: Duration) {
        for (slot, d) in [(&mut self.lex, t_lex), (&mut self.parse, t_parse), (&mut self.vm, t_vm)] {
            slot.0 += d;
            if d > slot.1 { slot.1 = d; }
        }
        self.count += 1;
    }
    fn avg(&self, slot: &(Duration, Duration)) -> Duration {
        if self.count == 0 { Duration::ZERO } else { slot.0 / self.count as u32 }
    }
    fn print(&self) {
        eprintln!(
            "  perf  lex  avg={:>6}µs max={:>6}µs  parse avg={:>6}µs max={:>6}µs  vm avg={:>6}µs max={:>6}µs",
            self.avg(&self.lex).as_micros(), self.lex.1.as_micros(),
            self.avg(&self.parse).as_micros(), self.parse.1.as_micros(),
            self.avg(&self.vm).as_micros(), self.vm.1.as_micros(),
        );
    }
}

enum Outcome { Crash, ParseErr, VmErr, Timeout, Clean(u128, Duration, Duration, Duration) }

fn run_once(src: &str) -> Outcome {
    let src = if src.len() > MAX_LEN { src[..MAX_LEN].to_string() } else { src.to_string() };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let outcome = match panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let t0 = Instant::now();
            let (tokens, _) = lex(&src);
            let t_lex = t0.elapsed();

            let t1 = Instant::now();
            let (chunk, errs) = Parser::new(&src, tokens.into_iter()).parse();
            let t_parse = t1.elapsed();

            let bm = opcode_bitmap(&chunk);

            let t2 = Instant::now();
            let ok = VM::new(&chunk).run().is_ok();
            let t_vm = t2.elapsed();

            (errs.is_empty(), ok, bm, t_lex, t_parse, t_vm)
        })) {
            Err(_) => Outcome::Crash,
            Ok((false, ..)) => Outcome::ParseErr,
            Ok((true, false, ..)) => Outcome::VmErr,
            Ok((true, true, bm, tl, tp, tv)) => Outcome::Clean(bm, tl, tp, tv),
        };
        let _ = tx.send(outcome);
    });
    rx.recv_timeout(VM_TIMEOUT).unwrap_or(Outcome::Timeout)
}

struct Corpus { entries: Vec<String>, seen: u128 }

impl Corpus {
    fn new() -> Self {
        Self { entries: load_seeds(), seen: 0 }
    }
    fn pick(&self, rng: &mut Rng) -> &str {
        &self.entries[rng.usize_in(self.entries.len())]
    }
    fn add(&mut self, src: String, bm: u128) -> bool {
        let fresh = bm & !self.seen;
        if fresh != 0 { self.seen |= fresh; self.entries.push(src); true } else { false }
    }
}

struct Stats { iters: u64, crashes: u64, adds: u64, timeouts: u64, start: Instant }

impl Stats {
    fn new() -> Self { Self { iters: 0, crashes: 0, adds: 0, timeouts: 0, start: Instant::now() } }
    fn print(&self, corpus: usize, perf: &Perf) {
        let s = self.start.elapsed().as_secs_f64().max(0.001);
        eprintln!("[{:7.1}s] iters={:<9} {:.0}/s  crashes={}  timeouts={}  corpus={}  new_cov={}",
            s, self.iters, self.iters as f64 / s, self.crashes, self.timeouts, corpus, self.adds);
        perf.print();
    }
}

fn main() {
    std::fs::create_dir_all(SAVE_DIR).expect("failed to create crashes dir");
    let mut rng = Rng::new();
    let mut corpus = Corpus::new();
    let mut stats = Stats::new();
    let mut perf = Perf::new();

    eprintln!("fuzzer start — seeds={} max_input={MAX_LEN} save={SAVE_DIR}",
        corpus.entries.len());

    loop {
        let base = corpus.pick(&mut rng).to_string();
        let input = mutate(&base, &corpus.entries, &mut rng);

        match run_once(&input) {
            Outcome::Crash => {
                stats.crashes += 1;
                let path = format!("{SAVE_DIR}/crash_{:06}.py", stats.crashes);
                let _ = std::fs::write(&path, &input);
                eprintln!("\n[CRASH #{:06}] -> {path}\n  {:?}\n", stats.crashes, &input[..input.len().min(120)]);
            }
            Outcome::Clean(bm, t_lex, t_parse, t_vm) => {
                perf.record(t_lex, t_parse, t_vm);
                let total = t_lex + t_parse + t_vm;
                if total > SLOW_THRESHOLD {
                    let path = format!("{SAVE_DIR}/slow_{:06}.py", stats.iters);
                    let _ = std::fs::write(&path, &input);
                    eprintln!("\n[SLOW {}ms] -> {path}\n  lex={}µs parse={}µs vm={}µs\n",
                        total.as_millis(), t_lex.as_micros(), t_parse.as_micros(), t_vm.as_micros());
                }
                if corpus.add(input, bm) { stats.adds += 1; }
            }
            Outcome::Timeout => { stats.timeouts += 1; }
            _ => {}
        }

        stats.iters += 1;
        if stats.iters.is_multiple_of(PRINT_INTERVAL) {
            stats.print(corpus.entries.len(), &perf);
        }
        if stats.start.elapsed().as_secs() >= MAX_SECS { break; }
    }

    stats.print(corpus.entries.len(), &perf);
    eprintln!("done — {} iters in {}s", stats.iters, stats.start.elapsed().as_secs());
}
