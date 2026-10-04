//! The server that ships the game, and the whole of the container image.
//!
//! Small on purpose. There is a reverse proxy in front of this terminating
//! TLS, so what is left is one job: hand over twelve files that were fixed
//! when this binary was built, with the right cache headers on each, and
//! compressed when the client will take them that way.
//!
//! The compression is deliberately this side of the proxy. That proxy is a
//! shared gateway with no compression filter on it, and adding one there would
//! change what four other zones get; doing it here is bounded by this image.
//! Since every file is fixed at build time it is done once at startup, so it
//! is off the request path entirely: see [`Ready`].
//!
//! # The site is inside the binary
//!
//! Every file is an `include_bytes!`, so the image is one executable on top of
//! `scratch`: no filesystem, no shell, and no path handling at all. A request
//! path is matched against a table of names rather than joined onto a root
//! directory, which is the entire class of traversal bugs gone rather than
//! guarded against.
//!
//! It also means the assets cannot drift from the code that serves them. A
//! half-deployed image is not a state this can be in.
//!
//! # Why the URLs carry a hash
//!
//! A returning player must never end up running this build's HTML against the
//! last build's wasm. The two are not independent: the front end and the
//! engine agree about an ABI, and a mismatched pair is a blank page.
//!
//! So everything but the HTML is served under `/a/<fingerprint>/`, where the
//! fingerprint covers every byte of the site. Those URLs can be cached
//! forever, because a changed file is a changed URL rather than the same URL
//! with new contents. The HTML is the one thing that is never cached, and it
//! is what names the fingerprint, so a reload picks up a new build whole or
//! not at all.
//!
//! One fingerprint over the whole site rather than one per file. Per file
//! would let a build reuse the unchanged parts, which for a site of this size
//! saves a quarter of a megabyte, and it would mean rewriting the import
//! specifiers inside the modules, since they refer to each other by name. A
//! single prefix leaves every relative path in the site working exactly as it
//! does in development: `./engine.js` from a module under the prefix resolves
//! under the prefix, and `../twiddlygems.wasm` does too.
//!
//! # What the place this runs allows it
//!
//! Three of these are constraints on the code rather than on a manifest, so
//! they are here rather than only in the repository that deploys it.
//!
//! - **There is no egress at all, not even DNS.** Nothing here may reach the
//!   network outward, and nothing the page loads may either: a font from a
//!   CDN or a script from an analytics service would not be slow, it would
//!   simply never arrive. Everything the site needs is inside this binary,
//!   and [`POLICY`] is what keeps it that way.
//! - **The root filesystem is read only**, with nothing mounted over it. That
//!   is affordable because this writes nothing, ever; a cache file or a log on
//!   disk would need somebody to add a volume for it.
//! - **Startup has about thirty seconds** before the container is given up on
//!   and restarted. Compressing the site at boot puts work on that path for
//!   the first time, and at a few hundred kilobytes it is a few milliseconds,
//!   but it is the ceiling that work has.
//!
//! # What it deliberately does not do
//!
//! No keep-alive: every response says `Connection: close`. A proxy opening a
//! connection per request over a loopback interface costs almost nothing, and
//! it means there is no request framing state to get wrong. It also means a
//! shutdown has no pooled connections to cut, which is most of why the drain
//! below can be as simple as it is.
//!
//! No access log. One line at startup and nothing per request: pod logs where
//! this runs are shipped and kept for six months, so logging a line per
//! request would be a storage decision rather than a verbosity one.
//!
//! No metrics endpoint. What the orchestrator already knows, plus the two
//! synthetic probes described under [`route_to`], tell "down", "serving the
//! wrong thing" and "restarting in a loop" apart between them.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use twiddlygems::deflate;
use twiddlygems::VERSION;

/// Whether this process has been told to stop.
///
/// Set from a signal handler, so nothing here may allocate, lock or call
/// anything that might: a store to an atomic is about the whole of what is
/// allowed in one, and it is all this needs.
static DRAINING: AtomicBool = AtomicBool::new(false);

// The two signals an orchestrator sends to end a container, declared the way
// this repository declares the rest of its C boundary. A crate could do this
// with an attribute, and the empty dependency list is worth more than the
// attribute: see `src/ffi.rs` for the same reasoning about the wasm side.
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

extern "C" {
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
}

extern "C" fn stop_soon(_signum: i32) {
    DRAINING.store(true, Ordering::SeqCst);
}

/// How long to keep answering after being told to stop.
///
/// The point of waiting at all is that being signalled and being taken out of
/// a load balancer's list are two different events with no ordering between
/// them: for a few seconds after the signal, requests are still arriving from
/// something that has not yet been told. Exiting immediately cuts those.
///
/// Elsewhere this is done with a `preStop` hook running `sleep`, which cannot
/// work here because there is no shell and no `sleep` in the image. So the
/// binary does it, which is what that hook was imitating anyway.
///
/// It must stay comfortably under whatever grace period the thing stopping
/// this allows, or the wait is pointless: the process would be killed part way
/// through it, which is the ungraceful shutdown this exists to avoid, arrived
/// at by way of trying to avoid it. Five against a thirty second grace period
/// and a ten second `docker stop` both leave room.
fn drain_for() -> Duration {
    let seconds = std::env::var("DRAIN_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(5);
    Duration::from_secs(seconds)
}

/// One file of the site, under the name the page asks for it by.
struct Asset {
    path: &'static str,
    body: &'static [u8],
    mime: &'static str,
}

const JS: &str = "text/javascript; charset=utf-8";

/// Everything but the page itself, which is built rather than served as it is.
///
/// Listed rather than discovered, because there is no filesystem here to
/// discover it from. `every_file_of_the_site_is_served` keeps the list honest
/// against the directory it came from.
const ASSETS: &[Asset] = &[
    Asset {
        path: "css/style.css",
        body: include_bytes!("../../web/css/style.css"),
        mime: "text/css; charset=utf-8",
    },
    Asset { path: "js/main.js", body: include_bytes!("../../web/js/main.js"), mime: JS },
    Asset { path: "js/engine.js", body: include_bytes!("../../web/js/engine.js"), mime: JS },
    Asset {
        path: "js/archipelago.js",
        body: include_bytes!("../../web/js/archipelago.js"),
        mime: JS,
    },
    Asset { path: "js/render.js", body: include_bytes!("../../web/js/render.js"), mime: JS },
    Asset { path: "js/hud.js", body: include_bytes!("../../web/js/hud.js"), mime: JS },
    Asset { path: "js/input.js", body: include_bytes!("../../web/js/input.js"), mime: JS },
    Asset { path: "js/audio.js", body: include_bytes!("../../web/js/audio.js"), mime: JS },
    Asset { path: "js/sounds.js", body: include_bytes!("../../web/js/sounds.js"), mime: JS },
    Asset {
        path: "twiddlygems.wasm",
        body: include_bytes!("../../web/twiddlygems.wasm"),
        mime: "application/wasm",
    },
];

/// The page, before its references are pointed at the fingerprinted prefix.
const INDEX: &[u8] = include_bytes!("../../web/index.html");

/// The browser-tab icon, which is the one file not under the prefix.
///
/// A browser asks for `/favicon.ico` at the root whatever a page says, so
/// there is no fingerprinting it. It is committed rather than built, unlike
/// the module beside it, because it is drawn out of the game's own gem painter
/// by `make favicon` and changes approximately never.
const FAVICON: &[u8] = include_bytes!("../../web/favicon.ico");

/// What the page says when it reaches for the stylesheet and the first module.
///
/// Matched exactly rather than by pattern, and both must be found: a rewrite
/// that silently matched nothing would serve a page asking for URLs that are
/// not there, which is a blank screen rather than a stale one.
const REFERENCES: [(&str, &str); 2] = [("href=\"css/", "href=\"{}css/"), ("src=\"js/", "src=\"{}js/")];

/// Where the fingerprinted copies live. Short, because it is on every URL.
///
/// **This, the sixteen hex digits after it, and `js/main.js` are an interface
/// rather than an implementation detail.** A synthetic probe on the cluster
/// this runs on asserts that the page contains
/// `src="/a/[0-9a-f]{16}/js/main.js"` and pages somebody when it does not,
/// because that one string proves three things a status code cannot: that a
/// shared gateway routed to this application rather than to one of the others
/// on the same address, that the binary rewrote the page instead of serving
/// the raw template, and that the URL the browser is about to fetch resolves.
///
/// So changing the prefix, the width of the fingerprint, or the name of the
/// entry point is a false alarm in the middle of the night for somebody.
/// Doable, and worth a word first. The same goes for `/healthz` answering 200
/// and the page living at `/`.
const PREFIX: &str = "a";

/// How long a client gets to send its request and take its answer.
///
/// There is a proxy on the other end of this, on a loopback interface, so
/// anything slow is something wrong rather than somebody's phone on a train.
const PATIENCE: Duration = Duration::from_secs(15);

/// The most request head we will read before giving up on a client.
const MOST_HEAD: usize = 8 * 1024;

/// One thing to hand over, in both the forms it can go out in.
///
/// The compressed copy is made once, when the process starts. Every file here
/// was fixed when the binary was built, so compressing per request would be
/// doing the same arithmetic over and over for an answer that cannot change.
/// A few hundred kilobytes of source takes a few milliseconds and about half a
/// megabyte of memory, once.
struct Ready {
    mime: &'static str,
    plain: &'static [u8],
    /// The same bytes gzipped, or nothing when that came out no smaller.
    ///
    /// Not every file is worth it: something already compressed, or short
    /// enough that the twenty byte wrapper outweighs what was saved, comes out
    /// bigger. Serving that would be slower at both ends for no reason.
    gzipped: Option<Vec<u8>>,
}

impl Ready {
    fn new(mime: &'static str, plain: &'static [u8]) -> Ready {
        let gzipped = deflate::gzip(plain);
        Ready {
            mime,
            plain,
            gzipped: (gzipped.len() < plain.len()).then_some(gzipped),
        }
    }

    /// Which copy to send, and what to say it is.
    fn pick(&self, may_compress: bool) -> (&[u8], Option<&'static str>) {
        match &self.gzipped {
            Some(gzipped) if may_compress => (gzipped, Some("gzip")),
            _ => (self.plain, None),
        }
    }
}

/// The site as it is actually served.
struct Site {
    /// Fingerprint of every byte, which is the cache-busting half of the URL.
    fingerprint: String,
    /// The page, pointed at that prefix. Owned rather than borrowed, because
    /// it is the one file that is built rather than shipped as it stands.
    index_html: Vec<u8>,
    index: Ready,
    favicon: Ready,
    assets: Vec<Ready>,
}

impl Site {
    fn build() -> Site {
        let fingerprint = fingerprint();
        let index_html = rewrite(INDEX, &fingerprint);
        // The page is the one thing not known at compile time, so its bytes
        // have to outlive this call to be handed out by reference. Leaked on
        // purpose: there is exactly one of them and it lives as long as the
        // process does.
        let leaked: &'static [u8] = Box::leak(index_html.clone().into_boxed_slice());
        Site {
            index: Ready::new("text/html; charset=utf-8", leaked),
            favicon: Ready::new("image/x-icon", FAVICON),
            assets: ASSETS.iter().map(|asset| Ready::new(asset.mime, asset.body)).collect(),
            fingerprint,
            index_html,
        }
    }

    /// The asset a fingerprinted path names, or nothing.
    ///
    /// The fingerprint has to be this build's. An old one is a request from a
    /// page this build did not serve, and answering it with today's file under
    /// yesterday's immutable URL would poison a cache with a lie.
    fn asset(&self, path: &str) -> Option<&Ready> {
        let rest = path.strip_prefix(&format!("/{PREFIX}/{}/", self.fingerprint))?;
        let at = ASSETS.iter().position(|asset| asset.path == rest)?;
        self.assets.get(at)
    }
}

/// Whether a client said it would take gzip.
///
/// The header is a list of codings with optional weights, and a weight of zero
/// is a refusal rather than a low preference, which is the one part of this
/// worth reading properly: `gzip;q=0` means *not gzip*, and treating it as an
/// offer sends somebody something they told us they could not read.
///
/// Everything else about the header is ignored on purpose. There is one coding
/// on offer here, so ordering preferences between several have nothing to
/// choose between, and `*` is taken as the yes it almost always is.
fn takes_gzip(header: &str) -> bool {
    for part in header.split(',') {
        let mut pieces = part.split(';').map(str::trim);
        let Some(coding) = pieces.next() else { continue };
        if !coding.eq_ignore_ascii_case("gzip") && coding != "*" {
            continue;
        }
        let refused = pieces.any(|piece| {
            piece
                .strip_prefix("q=")
                .or_else(|| piece.strip_prefix("Q="))
                .is_some_and(|q| q.parse::<f32>().is_ok_and(|weight| weight <= 0.0))
        });
        if !refused {
            return true;
        }
    }
    false
}

/// A number that changes when any byte of the site changes.
///
/// FNV-1a, which is nine lines and has no dependencies. This is a cache key
/// and not a signature: what it has to do is differ between two builds that
/// differ, and nobody gains anything by making it collide.
///
/// The path goes in alongside the body so that moving a file is a change even
/// when its contents are not, and a separator goes between them so that two
/// different splits of the same bytes cannot land on the same number.
fn fingerprint() -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(INDEX);
    for asset in ASSETS {
        eat(asset.path.as_bytes());
        eat(b"\0");
        eat(asset.body);
        eat(b"\0");
    }
    format!("{hash:016x}")
}

/// Points the page at the fingerprinted copies of what it asks for.
///
/// Panics when a reference is missing, which is the right time to find out:
/// this runs at startup and in `--selftest`, so a page whose shape has changed
/// fails the build rather than the player.
fn rewrite(html: &[u8], fingerprint: &str) -> Vec<u8> {
    let mut text = String::from_utf8(html.to_vec()).expect("the page is not valid UTF-8");
    let prefix = format!("/{PREFIX}/{fingerprint}/");
    for (from, to) in REFERENCES {
        assert!(text.contains(from), "the page has no {from} reference to point at the assets");
        text = text.replace(from, &to.replace("{}", &prefix));
    }
    text.into_bytes()
}

fn main() {
    let site = Site::build();

    if std::env::args().any(|arg| arg == "--selftest") {
        selftest(&site);
        return;
    }

    // SAFETY: both calls install a plain function that stores to an atomic and
    // returns. Done before anything is listening, so no request can be in
    // flight while the disposition changes.
    unsafe {
        signal(SIGTERM, stop_soon);
        signal(SIGINT, stop_soon);
    }

    let listener = listen();
    let where_ = listener.local_addr().map_or_else(|_| "?".to_string(), |at| at.to_string());
    println!(
        "twiddlygems-serve: {where_}, {} files, build {}",
        ASSETS.len() + 2,
        site.fingerprint,
    );

    // A fixed set of threads all accepting on the one socket, which the kernel
    // hands out one connection at a time. No channel and no queue: a thread is
    // either serving somebody or waiting in `accept`.
    //
    // Fixed rather than one per connection, because spawning on demand is an
    // invitation to be asked for a hundred thousand threads. A request here is
    // a memcpy of something already in memory, so this many is plenty, and
    // anything that does manage to tie one up is bounded by `PATIENCE`.
    let threads = thread::available_parallelism().map_or(8, |count| count.get().clamp(4, 32));
    let site = std::sync::Arc::new(site);
    for _ in 0..threads {
        let listener = listener.try_clone().expect("the listening socket cannot be shared");
        let site = std::sync::Arc::clone(&site);
        thread::spawn(move || accept_forever(&listener, &site));
    }

    // Every worker is accepting, so this thread waits for the signal instead
    // of serving. It cannot be one of the accepting threads: those are blocked
    // inside `accept` and would not notice a flag until somebody connected.
    while !DRAINING.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }
    // Readiness is already failing by now, because it reads the same flag.
    // What this wait is for is the requests still arriving from whatever has
    // not yet been told to stop sending them.
    println!("twiddlygems-serve: stopping, draining for {:?}", drain_for());
    thread::sleep(drain_for());
    // Zero: being asked to stop and then stopping is a success. A non-zero
    // exit here would be read as a crash and counted against the container.
    std::process::exit(0);
}

/// Opens the listening socket, on both address families where it can.
///
/// `::` rather than `0.0.0.0` by default, because a pod's address may be
/// either family and a server listening on only one of them is a connection
/// refused that reads as a crash. On Linux an unrestricted `::` socket accepts
/// IPv4 as well, so this is one socket rather than two.
///
/// The host and the port are parsed separately rather than pasted together.
/// `format!("{bind}:{port}")` turns `BIND=::` into `":::8080"`, which is not a
/// socket address anyone can parse, so the override had to be spelled `[::]`
/// and the trap was left for whoever set the variable next. Both spellings
/// work here.
fn listen() -> TcpListener {
    let bind = std::env::var("BIND").unwrap_or_else(|_| "::".to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8080);
    let host = bind.trim().trim_start_matches('[').trim_end_matches(']');
    let Ok(ip) = host.parse::<IpAddr>() else {
        eprintln!("twiddlygems-serve: BIND={bind} is not an address");
        std::process::exit(1);
    };

    match TcpListener::bind(SocketAddr::new(ip, port)) {
        Ok(listener) => listener,
        // A host with IPv6 switched off altogether, which a container engine
        // on somebody's laptop may well be. Worth carrying on for rather than
        // refusing to start, since the family that does work is the one being
        // asked for.
        Err(error) if ip.is_ipv6() && bind.is_empty() || ip.is_unspecified() && ip.is_ipv6() => {
            eprintln!("twiddlygems-serve: no IPv6 ({error}), falling back to IPv4 only");
            TcpListener::bind(SocketAddr::new(IpAddr::from([0, 0, 0, 0]), port)).unwrap_or_else(
                |error| {
                    eprintln!("twiddlygems-serve: cannot listen on port {port}: {error}");
                    std::process::exit(1);
                },
            )
        }
        Err(error) => {
            eprintln!("twiddlygems-serve: cannot listen on {host} port {port}: {error}");
            std::process::exit(1);
        }
    }
}

fn accept_forever(listener: &TcpListener, site: &Site) {
    for stream in listener.incoming() {
        match stream {
            // One bad connection is not worth ending a worker over.
            Ok(stream) => serve(stream, site),
            Err(error) => eprintln!("twiddlygems-serve: {error}"),
        }
    }
}

fn serve(mut stream: TcpStream, site: &Site) {
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let _ = stream.set_write_timeout(Some(PATIENCE));
    let _ = stream.set_nodelay(true);

    let Some(request) = read_request(&mut stream) else {
        let _ = reply(&mut stream, 400, PLAIN, b"bad request", None, None, false);
        return;
    };
    let (method, path, gzip_ok) = request;

    // HEAD answers exactly as GET does, minus the body, so a proxy asking
    // about a file gets the same headers it would cache.
    let body_wanted = match method.as_str() {
        "GET" => true,
        "HEAD" => false,
        _ => {
            let _ = reply(&mut stream, 405, PLAIN, b"method not allowed", None, None, false);
            return;
        }
    };

    let route = path.split(['?', '#']).next().unwrap_or("/");
    let _ = route_to(&mut stream, site, route, gzip_ok, body_wanted);
}

fn route_to(
    stream: &mut TcpStream,
    site: &Site,
    route: &str,
    gzip_ok: bool,
    body: bool,
) -> std::io::Result<()> {
    // The page, which is never cached. It names this build's prefix, so the
    // one thing that must always be fresh is the one thing that says which
    // build a player is running.
    if route == "/" || route == "/index.html" {
        return send(stream, &site.index, gzip_ok, NEVER, body);
    }
    // Anything under this build's prefix, which can be kept forever.
    if let Some(asset) = site.asset(route) {
        return send(stream, asset, gzip_ok, FOREVER, body);
    }
    // Two probes, and the difference between them is the whole of how this
    // shuts down without cutting anybody off.
    //
    // `/healthz` says the process is alive. It must keep saying so while the
    // process is draining, or a liveness check would kill a container that is
    // deliberately finishing its work and turn every rollout into a restart
    // loop.
    if route == "/healthz" {
        return reply(stream, 200, PLAIN, b"ok", Some(NEVER), None, body);
    }
    // `/readyz` says it should still be sent new work, and stops the moment a
    // signal arrives. That is what takes this out of a load balancer's list
    // before it goes, which is the half of a graceful shutdown that cannot be
    // done from inside the process any other way.
    if route == "/readyz" {
        let stopping = DRAINING.load(Ordering::SeqCst);
        let (status, said) = if stopping { (503, &b"draining"[..]) } else { (200, &b"ok"[..]) };
        return reply(stream, status, PLAIN, said, Some(NEVER), None, body);
    }
    // The one file that cannot be fingerprinted, because a browser asks for it
    // at the root whatever the page says. A day is the compromise the lack of
    // a fingerprint forces: long enough that nobody fetches it twice in a
    // session, short enough that changing it reaches people.
    if route == "/favicon.ico" {
        return send(stream, &site.favicon, gzip_ok, A_WHILE, body);
    }
    reply(stream, 404, PLAIN, b"not found", Some(NEVER), None, body)
}

/// Hands over one file, in whichever form the client said it would take.
fn send(
    stream: &mut TcpStream,
    ready: &Ready,
    gzip_ok: bool,
    cache: &str,
    body: bool,
) -> std::io::Result<()> {
    let (bytes, encoding) = ready.pick(gzip_ok);
    reply(stream, 200, ready.mime, bytes, Some(cache), encoding, body)
}

/// For the page and for everything that is not one of this build's files.
const NEVER: &str = "no-cache";
/// For a fingerprinted URL, which cannot ever mean a different file.
const FOREVER: &str = "public, max-age=31536000, immutable";
/// For the icon, which has no fingerprint to make a longer promise with.
const A_WHILE: &str = "public, max-age=86400";

const PLAIN: &str = "text/plain; charset=utf-8";

/// What the page is allowed to do, which is almost nothing.
///
/// A game with no third-party anything can afford a policy most sites cannot:
/// everything it loads is its own, there is no analytics, no font service and
/// no embedded frame, so the default is nothing and each exception is a thing
/// this page actually does.
///
/// Two of those exceptions are load-bearing and neither is obvious:
///
/// - **`wasm-unsafe-eval`**. Instantiating a WebAssembly module counts as
///   evaluating code. Without this the policy looks perfectly sensible, the
///   page loads, every module loads, and the board never appears: exactly the
///   failure the fingerprinting exists to prevent, arrived at another way.
/// - **`ws:` and `wss:` in `connect-src`**. A multiworld lives on whatever
///   host the player types into the connect screen, so the set of servers this
///   may talk to cannot be written down here. Narrowing this to `'self'`
///   leaves the solo game working perfectly and quietly breaks Archipelago,
///   which is the sort of thing found weeks later.
const POLICY: &str = "default-src 'none'; \
     script-src 'self' 'wasm-unsafe-eval'; \
     style-src 'self'; \
     img-src 'self'; \
     connect-src 'self' ws: wss:; \
     base-uri 'none'; \
     form-action 'none'; \
     frame-ancestors 'none'";

/// Reads the method, the path, and whether the client takes gzip.
///
/// Nothing else in a request matters here: no cookies, no ranges, no
/// conditional requests. The head is read to its end all the same, so a client
/// that sent one is not answered mid-sentence, and it is capped so a client
/// that never stops sending cannot make us hold it all.
fn read_request(stream: &mut TcpStream) -> Option<(String, String, bool)> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if head.len() >= MOST_HEAD {
            return None;
        }
        match stream.read(&mut byte) {
            Ok(0) => return None,
            Ok(_) => head.push(byte[0]),
            Err(_) => return None,
        }
        if head.ends_with(b"\r\n\r\n") || head.ends_with(b"\n\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    // Header names are case insensitive, and the one being looked for is
    // spelled differently by enough clients to be worth not caring.
    let gzip_ok = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("accept-encoding"))
        .is_some_and(|(_, value)| takes_gzip(value));
    Some((method, path, gzip_ok))
}

fn reply(
    stream: &mut TcpStream,
    status: u16,
    mime: &str,
    body: &[u8],
    cache: Option<&str>,
    encoding: Option<&str>,
    send_body: bool,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {mime}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         X-Content-Type-Options: nosniff\r\n",
        body.len(),
    );
    if let Some(cache) = cache {
        head.push_str(&format!("Cache-Control: {cache}\r\n"));
    }
    if let Some(encoding) = encoding {
        head.push_str(&format!("Content-Encoding: {encoding}\r\n"));
    }
    // On everything that has two forms, whether or not this particular answer
    // used the compressed one. A cache between here and the player keys on
    // this: without it, whichever form the first client asked for is what
    // every client behind that cache gets, including the ones that cannot read
    // it.
    if cache.is_some() {
        head.push_str("Vary: Accept-Encoding\r\n");
    }
    // On the page only. These govern what a document may do, and every other
    // response here is something a document loaded rather than a document.
    if mime.starts_with("text/html") {
        head.push_str(&format!("Content-Security-Policy: {POLICY}\r\n"));
        head.push_str("Referrer-Policy: no-referrer\r\n");
    }
    // On everything, including the assets: nothing here is meant to be pulled
    // into somebody else's page.
    head.push_str("Cross-Origin-Resource-Policy: same-origin\r\n");
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if send_body {
        stream.write_all(body)?;
    }
    stream.flush()
}

/// Checks the site the binary is carrying, for the image build to run.
///
/// A scratch image has no test runner in it, so this is the only thing that
/// can say the thing about to be shipped is coherent. It is not a substitute
/// for the tests below; it is the part of them that can be run against the
/// artifact rather than against the source.
fn selftest(site: &Site) {
    assert_eq!(site.fingerprint.len(), 16, "the fingerprint is the wrong shape");
    assert!(!site.index_html.is_empty(), "the page is empty");

    let page = String::from_utf8(site.index_html.clone()).expect("the page is not UTF-8");
    let prefix = format!("/{PREFIX}/{}/", site.fingerprint);
    assert!(page.contains(&format!("{prefix}js/main.js")), "the page does not load the game");
    assert!(page.contains(&format!("{prefix}css/style.css")), "the page has no stylesheet");

    for asset in ASSETS {
        assert!(!asset.body.is_empty(), "{} is empty", asset.path);
        let found = site.asset(&format!("{prefix}{}", asset.path));
        assert!(found.is_some(), "{} is not reachable at its own URL", asset.path);
    }
    assert!(site.asset(&format!("/{PREFIX}/0000000000000000/js/main.js")).is_none());

    // The one file that is not source, and the one most likely to be missing
    // or stale: a module built for the wrong target is still bytes.
    let wasm = ASSETS.iter().find(|asset| asset.path == "twiddlygems.wasm").expect("no module");
    assert_eq!(&wasm.body[..4], b"\0asm", "twiddlygems.wasm is not a WebAssembly module");
    assert!(wasm.body.len() > 50_000, "the module is too small to be the game");

    // And that it is *this* build of the game. The module is compiled
    // separately and embedded from disk, so a stale one is a server shipping a
    // game it does not describe, which nothing else here would notice: a
    // module from last week is still a module, still the right size, and still
    // plays.
    let stamp = VERSION.as_bytes();
    assert!(
        wasm.body.windows(stamp.len()).any(|at| at == stamp),
        "the module is from a different build than this server, which says {VERSION}",
    );

    // The icon, which is the one file nothing else here would notice going
    // missing: the page would load, the game would play, and the tab would
    // show whatever a browser shows when there is nothing.
    assert_eq!(&site.favicon.plain[..4], &[0, 0, 1, 0], "favicon.ico is not an icon");

    // And that the compressed copies were actually made. A build that quietly
    // stopped compressing would look exactly like a working one from here.
    let squeezed: usize = site.assets.iter().filter(|ready| ready.gzipped.is_some()).count();
    assert!(squeezed >= ASSETS.len() - 1, "only {squeezed} of the files compressed");

    let plain: usize = site.assets.iter().map(|ready| ready.plain.len()).sum();
    let small: usize = site
        .assets
        .iter()
        .map(|ready| ready.gzipped.as_ref().map_or(ready.plain.len(), Vec::len))
        .sum();
    // The version is printed as well as checked, so the image build can read
    // it out of this line rather than grepping a binary, and so a deployment's
    // own logs say which build is answering.
    println!(
        "selftest: ok, {} files, {plain} bytes, {small} compressed, build {}, version {VERSION}",
        ASSETS.len() + 2,
        site.fingerprint,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_of_the_site_is_served() {
        // The table above is written by hand because there is no filesystem in
        // the image to walk. This is the thing that stops it going stale: a
        // module added to the page and forgotten here would be a blank screen
        // in production and nothing at all in development, where the files are
        // served off disk.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
        let mut on_disk: Vec<String> = Vec::new();
        for directory in ["css", "js"] {
            for entry in std::fs::read_dir(root.join(directory)).expect("the site is not there") {
                let name = entry.expect("unreadable").file_name().to_string_lossy().to_string();
                on_disk.push(format!("{directory}/{name}"));
            }
        }
        on_disk.sort();
        let mut served: Vec<String> = ASSETS
            .iter()
            .map(|asset| asset.path.to_string())
            .filter(|path| path.contains('/'))
            .collect();
        served.sort();
        assert_eq!(served, on_disk, "the served list and the site have drifted apart");
    }

    #[test]
    fn the_page_is_pointed_at_the_fingerprinted_copies() {
        let site = Site::build();
        let page = String::from_utf8(site.index_html.clone()).unwrap();
        assert!(page.contains(&format!("src=\"/a/{}/js/main.js\"", site.fingerprint)));
        assert!(page.contains(&format!("href=\"/a/{}/css/style.css\"", site.fingerprint)));
        // And nothing is left pointing at the bare paths, which a proxy would
        // answer with a 404 rather than with the file.
        assert!(!page.contains("src=\"js/"), "a module is still asked for by its bare path");
        assert!(!page.contains("href=\"css/"), "the stylesheet is still asked for by its bare path");
    }

    #[test]
    fn only_this_build_s_fingerprint_opens_the_assets() {
        // An immutable URL is a promise that it will never mean a different
        // file. Answering yesterday's prefix with today's bytes would break
        // exactly that promise, in a cache, where it cannot be taken back.
        let site = Site::build();
        let good = format!("/a/{}/js/main.js", site.fingerprint);
        assert!(site.asset(&good).is_some());
        assert!(site.asset("/a/deadbeefdeadbeef/js/main.js").is_none());
        assert!(site.asset("/js/main.js").is_none(), "the bare path is served as well");
        // And nothing outside the table, however it is asked for.
        assert!(site.asset(&format!("/a/{}/../../etc/passwd", site.fingerprint)).is_none());
        assert!(site.asset(&format!("/a/{}/js/../js/main.js", site.fingerprint)).is_none());
    }

    #[test]
    fn the_fingerprint_follows_the_bytes() {
        // Two runs of the same build agree, or every deploy would invalidate
        // every cache for no reason.
        assert_eq!(fingerprint(), fingerprint());
        assert_eq!(fingerprint().len(), 16);
        // And a changed byte changes it, which is the whole job.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in b"twiddly" {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let mut other: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in b"twiddlz" {
            other ^= *byte as u64;
            other = other.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_ne!(hash, other, "the hash does not see a changed byte");
    }

    #[test]
    fn the_selftest_passes_on_the_site_this_binary_carries() {
        selftest(&Site::build());
    }

    #[test]
    fn a_client_is_only_sent_gzip_when_it_said_it_would_take_it() {
        assert!(takes_gzip("gzip"));
        assert!(takes_gzip("gzip, deflate, br"));
        assert!(takes_gzip("deflate, gzip;q=1.0, *;q=0.5"));
        assert!(takes_gzip("*"));
        assert!(takes_gzip("GZIP"), "the coding name is case insensitive");
        assert!(!takes_gzip(""));
        assert!(!takes_gzip("identity"));
        assert!(!takes_gzip("deflate, br"));
        // A weight of zero is a refusal rather than a low preference. Reading
        // it as an offer sends somebody exactly what they said they could not
        // read, and the header they said it in is the one nobody tests with.
        assert!(!takes_gzip("gzip;q=0"));
        assert!(!takes_gzip("gzip;q=0.0"));
        assert!(!takes_gzip("*;q=0"));
        assert!(!takes_gzip("gzip;q=0, deflate"));
        // Refusing gzip while allowing everything else still leaves the star.
        assert!(takes_gzip("gzip;q=0, *"));
        // And a weight that is present but tiny is still a yes.
        assert!(takes_gzip("gzip;q=0.001"));
    }

    #[test]
    fn what_goes_out_compressed_comes_from_the_same_bytes() {
        let site = Site::build();
        for (asset, ready) in ASSETS.iter().zip(&site.assets) {
            let (plain, no_encoding) = ready.pick(false);
            assert_eq!(plain, asset.body, "{} is not itself uncompressed", asset.path);
            assert_eq!(no_encoding, None, "{} claimed an encoding it has not got", asset.path);

            let (sent, encoding) = ready.pick(true);
            match &ready.gzipped {
                Some(gzipped) => {
                    assert_eq!(encoding, Some("gzip"));
                    assert_eq!(sent, &gzipped[..]);
                    assert!(sent.len() < asset.body.len(), "{} grew", asset.path);
                    assert_eq!(&sent[..3], &[0x1f, 0x8b, 8], "{} is not gzip", asset.path);
                }
                // A file that did not compress is handed over as it is rather
                // than with a wrapper that made it bigger.
                None => assert_eq!(encoding, None),
            }
        }
    }

    #[test]
    fn the_page_is_allowed_to_start_the_game_and_reach_a_multiworld() {
        // Both of these have the same failure: a policy that reads as sensible
        // and a game that does not run. Held to here so that tightening the
        // policy has to be done knowingly.
        assert!(
            POLICY.contains("'wasm-unsafe-eval'"),
            "without this the module cannot be instantiated and the board never appears",
        );
        assert!(
            POLICY.contains("connect-src 'self' ws: wss:"),
            "without this a multiworld on any host the player types is blocked",
        );
        assert!(POLICY.starts_with("default-src 'none'"), "the policy should deny by default");
    }

    #[test]
    fn the_address_a_container_is_given_is_one_it_can_parse() {
        // `::` unbracketed is the spelling anybody writing a manifest reaches
        // for, and pasting it onto a port gives `":::8080"`, which parses as
        // nothing. Both forms have to arrive at the same socket.
        for spelling in ["::", "[::]"] {
            let host = spelling.trim().trim_start_matches('[').trim_end_matches(']');
            let ip: IpAddr = host.parse().expect("should parse");
            assert!(ip.is_unspecified() && ip.is_ipv6(), "{spelling} did not come out as ::");
        }
        let ip: IpAddr = "0.0.0.0".parse().unwrap();
        assert!(ip.is_ipv4());
    }

    #[test]
    fn the_site_is_worth_compressing_at_all() {
        // Not a ratio to defend to the percent, but the whole reason this is
        // here. If the site ever stops shrinking by a useful amount, the
        // encoder has broken in a way that still produces valid output.
        let site = Site::build();
        let plain: usize = site.assets.iter().map(|ready| ready.plain.len()).sum();
        let small: usize = site
            .assets
            .iter()
            .map(|ready| ready.gzipped.as_ref().map_or(ready.plain.len(), Vec::len))
            .sum();
        assert!(
            small * 2 < plain,
            "the site only went from {plain} to {small} bytes, which is not worth doing",
        );
    }
}
