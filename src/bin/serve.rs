//! The server that ships the game, and the whole of the container image.
//!
//! Small on purpose. There is a reverse proxy in front of this doing TLS,
//! compression and whatever else the outside world needs, so what is left is
//! one job: hand over eleven files that were fixed when this binary was
//! built, with the right cache headers on each.
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
//! # What it deliberately does not do
//!
//! No keep-alive: every response says `Connection: close`. A proxy opening a
//! connection per request over a loopback interface costs almost nothing, and
//! it means there is no request framing state to get wrong.
//!
//! No compression either. The proxy in front does that, and doing it twice is
//! worse than doing it once.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

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

/// What the page says when it reaches for the stylesheet and the first module.
///
/// Matched exactly rather than by pattern, and both must be found: a rewrite
/// that silently matched nothing would serve a page asking for URLs that are
/// not there, which is a blank screen rather than a stale one.
const REFERENCES: [(&str, &str); 2] = [("href=\"css/", "href=\"{}css/"), ("src=\"js/", "src=\"{}js/")];

/// Where the fingerprinted copies live. Short, because it is on every URL.
const PREFIX: &str = "a";

/// How long a client gets to send its request and take its answer.
///
/// There is a proxy on the other end of this, on a loopback interface, so
/// anything slow is something wrong rather than somebody's phone on a train.
const PATIENCE: Duration = Duration::from_secs(15);

/// The most request head we will read before giving up on a client.
const MOST_HEAD: usize = 8 * 1024;

/// The site as it is actually served.
struct Site {
    /// Fingerprint of every byte, which is the cache-busting half of the URL.
    fingerprint: String,
    /// The page, pointed at that prefix.
    index: Vec<u8>,
}

impl Site {
    fn build() -> Site {
        let fingerprint = fingerprint();
        let index = rewrite(INDEX, &fingerprint);
        Site { fingerprint, index }
    }

    /// The asset a fingerprinted path names, or nothing.
    ///
    /// The fingerprint has to be this build's. An old one is a request from a
    /// page this build did not serve, and answering it with today's file under
    /// yesterday's immutable URL would poison a cache with a lie.
    fn asset(&self, path: &str) -> Option<&'static Asset> {
        let rest = path.strip_prefix(&format!("/{PREFIX}/{}/", self.fingerprint))?;
        ASSETS.iter().find(|asset| asset.path == rest)
    }
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

    let bind = std::env::var("BIND").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let listener = TcpListener::bind(format!("{bind}:{port}")).unwrap_or_else(|error| {
        eprintln!("twiddlygems-serve: cannot listen on {bind}:{port}: {error}");
        std::process::exit(1);
    });
    println!(
        "twiddlygems-serve: {bind}:{port}, {} files, build {}",
        ASSETS.len() + 1,
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
    for _ in 1..threads {
        let listener = listener.try_clone().expect("the listening socket cannot be shared");
        let site = std::sync::Arc::clone(&site);
        thread::spawn(move || accept_forever(&listener, &site));
    }
    accept_forever(&listener, &site);
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

    let Some((method, path)) = read_request(&mut stream) else {
        let _ = reply(&mut stream, 400, "text/plain; charset=utf-8", b"bad request", None, false);
        return;
    };

    // HEAD answers exactly as GET does, minus the body, so a proxy asking
    // about a file gets the same headers it would cache.
    let body_wanted = match method.as_str() {
        "GET" => true,
        "HEAD" => false,
        _ => {
            let _ = reply(
                &mut stream,
                405,
                "text/plain; charset=utf-8",
                b"method not allowed",
                None,
                false,
            );
            return;
        }
    };

    let route = path.split(['?', '#']).next().unwrap_or("/");
    let _ = route_to(&mut stream, site, route, body_wanted);
}

fn route_to(stream: &mut TcpStream, site: &Site, route: &str, body: bool) -> std::io::Result<()> {
    // The page, which is never cached. It names this build's prefix, so the
    // one thing that must always be fresh is the one thing that says which
    // build a player is running.
    if route == "/" || route == "/index.html" {
        return reply(stream, 200, "text/html; charset=utf-8", &site.index, Some(NEVER), body);
    }
    // Anything under this build's prefix, which can be kept forever.
    if let Some(asset) = site.asset(route) {
        return reply(stream, 200, asset.mime, asset.body, Some(FOREVER), body);
    }
    // Something for a load balancer to ask, answered without touching
    // anything, so it says this process is up rather than that the site is.
    if route == "/healthz" {
        return reply(stream, 200, "text/plain; charset=utf-8", b"ok", Some(NEVER), body);
    }
    // There is no icon, and saying so properly is worth three lines: a browser
    // asks for this on every first visit whatever the page says, and a 404 is
    // an answer it will come back and ask again. An empty 204 it can keep
    // means it asks once. The day there is an icon, it becomes an asset like
    // any other and this goes.
    if route == "/favicon.ico" {
        return reply(stream, 204, "image/x-icon", b"", Some(FOREVER), body);
    }
    reply(stream, 404, "text/plain; charset=utf-8", b"not found", Some(NEVER), body)
}

/// For the page and for everything that is not one of this build's files.
const NEVER: &str = "no-cache";
/// For a fingerprinted URL, which cannot ever mean a different file.
const FOREVER: &str = "public, max-age=31536000, immutable";

/// Reads the method and the path, and throws the rest away.
///
/// Nothing else in a request matters here: there is no content negotiation to
/// do, no cookies, no ranges. The head is read to its end all the same, so a
/// client that sent one is not answered mid-sentence, and it is capped so a
/// client that never stops sending cannot make us hold it all.
fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
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
    let mut first = text.lines().next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    Some((method, path))
}

fn reply(
    stream: &mut TcpStream,
    status: u16,
    mime: &str,
    body: &[u8],
    cache: Option<&str>,
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
    assert!(!site.index.is_empty(), "the page is empty");

    let page = String::from_utf8(site.index.clone()).expect("the page is not UTF-8");
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

    println!(
        "selftest: ok, {} files, {} bytes, build {}",
        ASSETS.len() + 1,
        site.index.len() + ASSETS.iter().map(|asset| asset.body.len()).sum::<usize>(),
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
        let page = String::from_utf8(site.index.clone()).unwrap();
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
}
