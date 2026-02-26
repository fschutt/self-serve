use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use std::sync::{Arc, Mutex};

mod transpiler;
mod dom;

use transpiler::Transpiler;
use dom::{Dom, DomNode};

type AppState = Arc<Mutex<State>>;

struct State {
    counter: i32,
}

#[derive(Clone)]
struct ServerContext {
    transpiler: Arc<Transpiler>,
    state: AppState,
}

// These three functions are the "source of truth" that gets transpiled to WASM.
// #[no_mangle] ensures the symbol survives into the binary so the transpiler
// can locate the machine code bytes at runtime.
// #[inline(never)] prevents the optimizer from inlining them away.
#[no_mangle]
#[inline(never)]
pub extern "C" fn increment_counter(value: i32) -> i32 {
    value + 1
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn decrement_counter(value: i32) -> i32 {
    value - 1
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn reset_counter(_value: i32) -> i32 {
    0
}

fn render_app(counter: i32) -> Dom {
    Dom {
        nodes: vec![
            DomNode::element("div", vec![("class", "container")], vec![
                DomNode::element("h1", vec![], vec![
                    DomNode::text("x64 \u{2192} WASM Server"),
                ]),
                DomNode::element("p", vec![("class", "subtitle")], vec![
                    DomNode::text("This server transpiles its own x86-64 machine code to WASM on startup."),
                    DomNode::text(" Each button click fetches the transpiled WASM, runs it in your browser, then syncs the server."),
                ]),
                DomNode::element("div", vec![("class", "counter-wrap")], vec![
                    DomNode::element("span", vec![("class", "counter-display"), ("id", "counter")], vec![
                        DomNode::text(&format!("{}", counter)),
                    ]),
                ]),
                DomNode::element("div", vec![("class", "buttons")], vec![
                    DomNode::element("button", vec![
                        ("onclick", "executeCallback('increment_counter')"),
                    ], vec![DomNode::text("+ Increment")]),
                    DomNode::element("button", vec![
                        ("onclick", "executeCallback('decrement_counter')"),
                    ], vec![DomNode::text("\u{2212} Decrement")]),
                    DomNode::element("button", vec![
                        ("onclick", "executeCallback('reset_counter')"),
                    ], vec![DomNode::text("\u{21ba} Reset")]),
                ]),
                DomNode::element("pre", vec![("id", "log"), ("class", "log")], vec![
                    DomNode::text("Click a button to execute transpiled WASM\u{2026}"),
                ]),
            ]),
        ],
    }
}

async fn index(ctx: web::Data<ServerContext>) -> impl Responder {
    let counter = ctx.state.lock().unwrap().counter;

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en"><head>
  <meta charset="utf-8">
  <title>x64 &rarr; WASM Server</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; }}
    body {{
      font-family: 'Courier New', monospace;
      background: #0d1117; color: #c9d1d9;
      margin: 0; padding: 40px 20px;
    }}
    .container {{ max-width: 620px; margin: 0 auto; }}
    h1 {{ font-size: 1.5em; color: #58a6ff; margin: 0 0 .3em; }}
    .subtitle {{ font-size: .78em; color: #8b949e; margin: 0 0 1.5em; line-height: 1.6; }}
    .counter-wrap {{ text-align: center; margin: .6em 0; }}
    .counter-display {{
      font-size: 5em; font-weight: bold; color: #f0f6fc;
      letter-spacing: -3px; display: inline-block;
      min-width: 3ch; transition: color .15s;
    }}
    .counter-display.flash {{ color: #3fb950; }}
    .buttons {{ display: flex; gap: 8px; justify-content: center; margin: .8em 0 1.2em; }}
    button {{
      padding: 9px 20px; background: #21262d;
      border: 1px solid #30363d; color: #c9d1d9;
      cursor: pointer; font-family: inherit; font-size: 13px;
      border-radius: 6px; transition: background .15s, border-color .15s;
    }}
    button:hover {{ background: #30363d; border-color: #58a6ff; color: #f0f6fc; }}
    button:disabled {{ opacity: .4; cursor: default; }}
    .log {{
      background: #161b22; border: 1px solid #30363d;
      border-radius: 6px; padding: 12px; font-size: 11.5px;
      color: #8b949e; min-height: 7em; white-space: pre-wrap;
      word-break: break-all; margin: 0;
    }}
    .log.ok  {{ border-color: #238636; color: #3fb950; }}
    .log.err {{ border-color: #6e2e1f; color: #f85149; }}
  </style>
</head>
<body>
{}
<script>
let counter = {};
const display = document.getElementById('counter');
const log = document.getElementById('log');
const buttons = document.querySelectorAll('button');

function setLog(text, cls) {{
  log.textContent = text;
  log.className = 'log' + (cls ? ' ' + cls : '');
}}
function flash() {{
  display.classList.add('flash');
  setTimeout(() => display.classList.remove('flash'), 300);
}}

async function executeCallback(fnName) {{
  buttons.forEach(b => b.disabled = true);
  try {{
    // ── Step 1: fetch the WASM module ──────────────────────────────────
    setLog(`Fetching /wasm/${{fnName}} …`);
    const t0 = performance.now();
    const resp = await fetch(`/wasm/${{fnName}}`);
    if (!resp.ok) throw new Error(`HTTP ${{resp.status}} fetching /wasm/${{fnName}}`);
    const wasmBytes = await resp.arrayBuffer();
    const fetchMs = (performance.now() - t0).toFixed(1);

    // ── Step 2: instantiate the WASM ──────────────────────────────────
    const {{ instance }} = await WebAssembly.instantiate(wasmBytes);
    const wasmFn = instance.exports[fnName];
    if (!wasmFn) throw new Error(`WASM has no export '${{fnName}}'`);

    // ── Step 3: run the transpiled x86-64 function locally ────────────
    const prev = counter;
    const next = wasmFn(counter);   // <- this runs machine code in your browser!

    counter = next;
    display.textContent = String(next);
    flash();

    let msg = `✓ Fetched ${{wasmBytes.byteLength}} B WASM in ${{fetchMs}} ms\n`;
    msg    += `  WASM exec: ${{fnName}}(${{prev}}) → ${{next}}\n`;

    // ── Step 4: sync server state ──────────────────────────────────────
    const syncResp = await fetch(`/execute/${{fnName}}`, {{ method: 'POST' }});
    const result = await syncResp.json();
    counter = result.value;
    display.textContent = String(result.value);

    msg += `  Server confirmed: ${{result.value}}`;
    setLog(msg, 'ok');
  }} catch (e) {{
    setLog(`Error: ${{e.message}}`, 'err');
    console.error(e);
  }} finally {{
    buttons.forEach(b => b.disabled = false);
  }}
}}
</script>
</body></html>"#,
        render_app(counter).to_html(),
        counter,
    );

    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

async fn get_wasm(
    path: web::Path<String>,
    ctx: web::Data<ServerContext>,
) -> impl Responder {
    let fn_name = path.into_inner();
    match ctx.transpiler.get_wasm_for_function(&fn_name) {
        Some(bytes) => HttpResponse::Ok()
            .content_type("application/wasm")
            .body(bytes),
        None => HttpResponse::NotFound()
            .body(format!("No transpiled WASM available for '{}'", fn_name)),
    }
}

async fn execute_callback(
    path: web::Path<String>,
    ctx: web::Data<ServerContext>,
) -> impl Responder {
    let fn_name = path.into_inner();
    let mut state = ctx.state.lock().unwrap();

    let new_value = match fn_name.as_str() {
        "increment_counter" => increment_counter(state.counter),
        "decrement_counter" => decrement_counter(state.counter),
        "reset_counter" => reset_counter(state.counter),
        _ => return HttpResponse::NotFound().body("Unknown callback"),
    };
    state.counter = new_value;

    HttpResponse::Ok()
        .content_type("application/json")
        .body(format!("{{\"value\":{}}}", new_value))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);

    println!("=== x64 \u{2192} WASM Server ===");
    println!("Arch: {}", std::env::consts::ARCH);
    println!("Reading own binary and transpiling functions to WASM…\n");

    let transpiler = Arc::new(Transpiler::new());
    let state = Arc::new(Mutex::new(State { counter: 0 }));
    let ctx = ServerContext { transpiler, state };

    println!("\nListening on http://0.0.0.0:{}", port);

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(ctx.clone()))
            .route("/", web::get().to(index))
            .route("/wasm/{fn_name}", web::get().to(get_wasm))
            .route("/execute/{fn_name}", web::post().to(execute_callback))
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
