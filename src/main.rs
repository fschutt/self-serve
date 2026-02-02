use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use std::collections::HashMap;
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

#[no_mangle]
pub extern "C" fn increment_counter(state_ptr: *mut State) -> i32 {
    unsafe {
        if state_ptr.is_null() {
            return 0;
        }
        let state = &mut *state_ptr;
        state.counter += 1;
        state.counter
    }
}

#[no_mangle]
pub extern "C" fn decrement_counter(state_ptr: *mut State) -> i32 {
    unsafe {
        if state_ptr.is_null() {
            return 0;
        }
        let state = &mut *state_ptr;
        state.counter -= 1;
        state.counter
    }
}

#[no_mangle]
pub extern "C" fn reset_counter(state_ptr: *mut State) -> i32 {
    unsafe {
        if state_ptr.is_null() {
            return 0;
        }
        let state = &mut *state_ptr;
        state.counter = 0;
        0
    }
}

fn render_app(state: &State) -> Dom {
    Dom {
        nodes: vec![
            DomNode::element("div", vec![
                ("class", "container"),
            ], vec![
                DomNode::element("h1", vec![], vec![
                    DomNode::text("x64 to WASM Counter"),
                ]),
                DomNode::element("p", vec![
                    ("class", "counter-display"),
                ], vec![
                    DomNode::text(&format!("Counter: {}", state.counter)),
                ]),
                DomNode::element("button", vec![
                    ("onclick", "executeCallback('increment_counter')"),
                ], vec![
                    DomNode::text("Increment"),
                ]),
                DomNode::element("button", vec![
                    ("onclick", "executeCallback('decrement_counter')"),
                ], vec![
                    DomNode::text("Decrement"),
                ]),
                DomNode::element("button", vec![
                    ("onclick", "executeCallback('reset_counter')"),
                ], vec![
                    DomNode::text("Reset"),
                ]),
            ]),
        ],
    }
}

async fn index(ctx: web::Data<ServerContext>) -> impl Responder {
    let state = ctx.state.lock().unwrap();
    let dom = render_app(&state);
    
    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>x64 to WASM Server</title>
    <style>
        body {{ font-family: Arial, sans-serif; max-width: 600px; margin: 50px auto; }}
        .container {{ text-align: center; }}
        .counter-display {{ font-size: 24px; margin: 20px 0; }}
        button {{ margin: 5px; padding: 10px 20px; font-size: 16px; cursor: pointer; }}
    </style>
    <script>
        async function executeCallback(fnName) {{
            try {{
                const wasmResponse = await fetch(`/wasm/${{fnName}}`);
                const wasmBytes = await wasmResponse.arrayBuffer();
                const wasmModule = await WebAssembly.instantiate(wasmBytes);
                
                // Execute the WASM function (it modifies server state)
                // For demo purposes, we just trigger it and reload
                await fetch(`/execute/${{fnName}}`, {{ method: 'POST' }});
                
                // Reload the page to show updated state
                window.location.reload();
            }} catch (e) {{
                console.error('Error executing callback:', e);
            }}
        }}
    </script>
</head>
<body>
{}
</body>
</html>"#,
        dom.to_html()
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
        Some(wasm_bytes) => HttpResponse::Ok()
            .content_type("application/wasm")
            .body(wasm_bytes),
        None => HttpResponse::NotFound().body("Function not found"),
    }
}

async fn execute_callback(
    path: web::Path<String>,
    ctx: web::Data<ServerContext>,
) -> impl Responder {
    let fn_name = path.into_inner();
    let mut state = ctx.state.lock().unwrap();
    
    match fn_name.as_str() {
        "increment_counter" => {
            increment_counter(&mut *state);
        }
        "decrement_counter" => {
            decrement_counter(&mut *state);
        }
        "reset_counter" => {
            reset_counter(&mut *state);
        }
        _ => return HttpResponse::NotFound().body("Unknown callback"),
    }
    
    HttpResponse::Ok().body("OK")
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port = std::env::var("RUN_AS_HTTP_SERVER")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);
    
    println!("Analyzing binary and transpiling functions...");
    let transpiler = Arc::new(Transpiler::new());
    
    let state = Arc::new(Mutex::new(State { counter: 0 }));
    
    let context = ServerContext {
        transpiler,
        state,
    };
    
    println!("Starting server on http://127.0.0.1:{}", port);
    println!("Available callbacks:");
    println!("  - increment_counter");
    println!("  - decrement_counter");
    println!("  - reset_counter");
    
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(context.clone()))
            .route("/", web::get().to(index))
            .route("/wasm/{fn_name}", web::get().to(get_wasm))
            .route("/execute/{fn_name}", web::post().to(execute_callback))
    })
    .bind(("127.0.0.1", port))?
    .run()
    .await
}
