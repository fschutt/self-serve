# x64 to WASM Server - Proof of Concept

This is a proof-of-concept implementation of a Rust server that transpiles its own x86-64 callback functions to WebAssembly at runtime.

## Concept

The core idea is to enable server-side rendering with client-side callbacks without writing JavaScript:

1. **Server-side DOM rendering**: The server maintains application state and renders a DOM tree
2. **Callback serialization**: C ABI callbacks are discovered via `dlsym`/`dladdr` and serialized as function names
3. **On-demand transpilation**: When the browser requests a callback, the server:
   - Locates the function in its own binary
   - Reads the x86-64 assembly instructions
   - Transpiles them to WebAssembly
   - Returns the WASM module to the browser
4. **Browser execution**: The browser downloads and executes the WASM, then triggers a state update

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Server Process (Rust Binary)                               │
│                                                             │
│  ┌────────────────┐         ┌──────────────────┐            │
│  │  App State     │────────▶│  DOM Renderer    │            │
│  │  (counter: i32)│         │  fn(State)->Dom  │            │
│  └────────────────┘         └──────────────────┘            │
│                                      │                      │
│                                      ▼                      │
│                             ┌──────────────────┐            │
│                             │  HTML + onclick  │            │
│                             │  callbacks       │            │
│                             └──────────────────┘            │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Transpiler                                          │   │
│  │  • Uses dlsym/dladdr to locate C functions           │   │
│  │  • Reads x86-64 assembly from binary                 │   │
│  │  • Converts to WASM instructions                     │   │
│  │  • Caches WASM modules                               │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                    ▲                    │
                    │                    │
              /execute/{fn}         /wasm/{fn}
                    │                    │
                    │                    ▼
         ┌─────────────────────────────────────┐
         │  Browser                            │
         │  • Downloads WASM on callback       │
         │  • Executes WASM                    │
         │  • Triggers server state update     │
         └─────────────────────────────────────┘
```

## Implementation Details

### Current PoC Status

This PoC demonstrates the concept with simplified transpilation:

- Server-side state management
- DOM tree rendering to HTML
- Callback discovery and serialization
- WASM module generation
- Browser-side WASM execution
- NOTE: Simplified transpilation (hand-coded WASM instead of actual x86-64 analysis)

### Callback Functions

The server exports C ABI functions that can be transpiled:

```rust
#[no_mangle]
pub extern "C" fn increment_counter(state_ptr: *mut State) -> i32 {
    unsafe {
        let state = &mut *state_ptr;
        state.counter += 1;
        state.counter
    }
}
```

### Real Implementation Notes

For a production implementation, you would:

1. **Binary Analysis**:
   - Use `object` crate to parse the ELF binary
   - Locate `.text` section and symbol table
   - Find function boundaries using debug symbols or heuristics

2. **Disassembly**:
   - Use `iced-x86` to disassemble x86-64 instructions
   - Build a control flow graph

3. **Transpilation**:
   - Map x86-64 registers to WASM locals
   - Convert instructions (mov → local.set, add → i32.add, etc.)
   - Handle calling conventions
   - Implement stack management

4. **Optimizations**:
   - Cache transpiled WASM modules
   - Inline small functions
   - Dead code elimination

## Usage

### Building

```bash
cargo build --release
```

### Running

```bash
# Default port 8080
cargo run --release

# Custom port
RUN_AS_HTTP_SERVER=3000 cargo run --release
```

### Testing

Open your browser to `http://127.0.0.1:8080`

Click the buttons to trigger callbacks:
- **Increment**: Calls `increment_counter` WASM
- **Decrement**: Calls `decrement_counter` WASM
- **Reset**: Calls `reset_counter` WASM

Each callback:
1. Fetches the WASM module from `/wasm/{fn_name}`
2. Instantiates and executes it
3. Sends a POST to `/execute/{fn_name}` to update server state
4. Reloads the page to show the new state

## API Endpoints

- `GET /` - Render the current application state as HTML
- `GET /wasm/{fn_name}` - Get transpiled WASM module for a callback
- `POST /execute/{fn_name}` - Execute a callback and update state

## Dependencies

- `actix-web` - HTTP server
- `wasm-encoder` - WASM bytecode generation
- `object` - ELF binary parsing (for real implementation)
- `iced-x86` - x86-64 disassembly (for real implementation)
- `libc` - dlsym/dladdr for symbol resolution

## Limitations & Future Work

### Current Limitations

1. **Simplified transpilation**: Hand-codes WASM instead of transpiling actual x86-64
2. **No register allocation**: Real x86-64 would need register mapping
3. **No memory model**: Doesn't handle pointers/heap properly
4. **Full page reload**: Could use websockets for live updates
5. **No optimization**: Each request re-generates HTML

### Potential Improvements

1. **Actual x86-64 transpilation**: 
   - Parse ELF binary with `object`
   - Disassemble with `iced-x86`
   - Build proper instruction mapper

2. **Smarter caching**:
   - Hash function bodies
   - Invalidate only when binary changes

3. **Better state management**:
   - Use linear memory in WASM
   - Share state via SharedArrayBuffer

4. **Incremental updates**:
   - DOM diffing
   - Websocket for push updates
   - Virtual DOM on client side

5. **Safety improvements**:
   - Validate transpiled WASM
   - Sandbox execution
   - Rate limiting

## Why This Approach?

Traditional web development requires:
- Server-side logic (Rust/Go/etc)
- Client-side logic (JavaScript)
- API layer between them
- State synchronization

This approach unifies:
- Single language (Rust)
- Single codebase
- Automatic serialization
- No manual API design
- Type-safe callbacks

Similar to how Azul.rs works for desktop, but adapted for web with WASM transpilation.

## License

MIT
