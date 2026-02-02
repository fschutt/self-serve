# x64 to WASM Transpilation - Executive Summary

## The Challenge

Convert x86-64 machine code from C callbacks to WebAssembly at runtime, handling:
- Jump labels and control flow
- Sub-function calls
- Register allocation
- Memory operations

## The Solution (TL;DR)

For simple callbacks (which is your use case), you can use a **simplified approach**:

1. **Extract function bytes** using `object` crate (parse ELF)
2. **Disassemble** using `iced-x86` crate
3. **Check complexity**:
   - No jumps? → Linear translation (fast)
   - Simple if/else? → Pattern matching (medium)
   - Complex control flow? → Full CFG analysis (slow)
4. **Translate** x86 instructions to WASM instructions
5. **Generate** WASM module using `wasm-encoder`

## Quick Reference

### Register Mapping
```
x86-64          →  WASM Local
----------------   -----------
RDI (param 1)   →  local 0
RSI (param 2)   →  local 1
RAX (return)    →  local N
RBX, RCX, etc.  →  local N+1, N+2...
```

### Common Instructions
```
mov rax, rdi    →  local.set $rax (local.get $rdi)
add rax, 1      →  local.set $rax (i64.add (local.get $rax) (i64.const 1))
mov rax, [rdi]  →  local.set $rax (i64.load (local.get $rdi))
mov [rdi], rax  →  i64.store (local.get $rdi) (local.get $rax)
ret             →  return (local.get $rax)
```

### Jump Handling
```
Simple if-else pattern:
  cmp rax, 0
  je .else
  <then code>
  jmp .done
.else:
  <else code>
.done:

Becomes:
  (if (i64.eqz (local.get $rax))
    (then <else code>)
    (else <then code>)
  )
```

## File Guide

### Core Implementation
- **`src/transpiler_real.rs`** - Full transpiler with CFG analysis
- **`examples/minimal_transpiler.rs`** - Minimal working example (~200 lines)

### Documentation
- **`STEP_BY_STEP.md`** - Complete walkthrough with example
- **`CONTROL_FLOW_GUIDE.md`** - Deep dive on jumps and labels
- **`IMPLEMENTATION.md`** - Original implementation notes

### Quick Start
- **`README.md`** - Overview and architecture
- **`QUICKSTART.md`** - How to use the PoC

## For Simple Callbacks

Most web callbacks are simple (no complex control flow). Use this fast path:

```rust
pub fn transpile_simple_callback(fn_name: &str) -> Vec<u8> {
    // 1. Extract from binary
    let (code, addr) = extract_function(fn_name)?;
    
    // 2. Disassemble
    let instrs = disassemble(code, addr)?;
    
    // 3. Check if simple (no jumps)
    if has_no_jumps(&instrs) {
        // Fast path: linear translation
        return transpile_linear(&instrs)?;
    }
    
    // 4. Otherwise, use CFG
    transpile_with_cfg(&instrs)?
}
```

**Expected performance:**
- Linear translation: ~1ms per function
- CFG analysis: ~10ms per function
- Cache WASM modules for instant subsequent calls

## Key Dependencies

```toml
iced-x86 = "1.21"      # x86-64 disassembler
object = "0.36"         # ELF parser
wasm-encoder = "0.222"  # WASM bytecode generation
libc = "0.2"           # dlsym/dladdr for symbol resolution
```

## Limitations & Workarounds

### Limitation: External Function Calls

```c
int callback() {
    return helper_function(42);  // Problem!
}
```

**Solutions:**
1. Inline small functions automatically
2. Add to WASM imports and stub on browser side
3. Recursively transpile entire call graph

### Limitation: Floating Point

```c
float callback(float x) {
    return x * 2.0f;  // Need f32/f64 support
}
```

**Solution:** Map XMM registers to f64 locals, use WASM f64 instructions.

### Limitation: System Calls

```c
int callback() {
    return getpid();  // Can't transpile this!
}
```

**Solution:** Detect syscalls, replace with WASM imports that call back to server.

## Production Checklist

- [ ] Validate generated WASM with `wasmparser`
- [ ] Cache transpiled modules (hash function bytes)
- [ ] Handle transpilation errors gracefully
- [ ] Set size limits (reject functions > 10KB)
- [ ] Add timeout for complex CFG analysis
- [ ] Monitor transpilation performance
- [ ] Test with sanitizers (ASan, MSan)
- [ ] Fuzz test with random x86-64 code

## Performance Tips

1. **Cache aggressively** - transpilation is expensive
2. **Transpile at startup** - not on first request
3. **Use release builds** - 10x faster than debug
4. **Batch transpilation** - parallel processing
5. **Inline small functions** - avoid WASM call overhead

## Testing Strategy

```rust
// Unit test: individual instructions
#[test]
fn test_add_instruction() {
    let code = [0x48, 0x01, 0xc7];  // add rdi, rax
    let wasm = transpile_instruction(&code);
    assert_valid_wasm(&wasm);
}

// Integration test: full function
#[test]
fn test_increment_callback() {
    let wasm = transpile_callback("increment_counter");
    
    // Execute and verify
    let result = run_wasm(&wasm, &[42]);
    assert_eq!(result, 43);
}

// Comparison test: x86 vs WASM
#[test]
fn test_equivalence() {
    let x86_result = execute_x86("callback", &args);
    let wasm_result = execute_wasm(transpile("callback"), &args);
    assert_eq!(x86_result, wasm_result);
}
```

## Example: End-to-End

```rust
// 1. Server starts
let transpiler = Transpiler::new("/path/to/server/binary");

// 2. Analyze callbacks at startup
transpiler.discover_callbacks();  // Uses dlsym
transpiler.transpile_all();       // Pre-transpile

// 3. Serve WASM on request
#[get("/wasm/{callback}")]
async fn get_wasm(name: Path<String>, ctx: Data<Transpiler>) -> impl Responder {
    match ctx.get_wasm(&name) {
        Some(wasm) => HttpResponse::Ok()
            .content_type("application/wasm")
            .body(wasm),
        None => HttpResponse::NotFound().finish(),
    }
}

// 4. Browser executes
// <button onclick="executeCallback('increment')">Click</button>
// 
// async function executeCallback(name) {
//     const wasm = await fetch(`/wasm/${name}`).then(r => r.arrayBuffer());
//     const module = await WebAssembly.instantiate(wasm);
//     module.instance.exports.callback();
//     await fetch(`/execute/${name}`, {method: 'POST'});
//     location.reload();
// }
```

## When to Use This Approach

✅ **Good for:**
- Simple callbacks (arithmetic, loads/stores, simple branches)
- Prototypes and demos
- Low-traffic applications
- When you control the C code

❌ **Not good for:**
- Complex algorithms with many jumps
- Functions that call lots of other functions
- High-frequency callbacks (overhead of transpilation)
- When you can just write JavaScript instead

## Alternatives to Consider

1. **Compile to WASM directly**: Use `wasm32` target instead of transpiling
2. **Use WebAssembly Interface Types**: Standard way to call WASM from JS
3. **Just use fetch()**: Traditional REST API approach
4. **Server-Sent Events**: Push updates from server

## Resources

- [WASM Spec](https://webassembly.github.io/spec/)
- [iced-x86 docs](https://docs.rs/iced-x86/)
- [wasm-encoder docs](https://docs.rs/wasm-encoder/)
- [Object crate docs](https://docs.rs/object/)

## Summary

For simple callbacks (your use case), the approach is:

1. Use `object` to find function in binary
2. Use `iced-x86` to disassemble
3. Map registers to WASM locals
4. Translate instructions one-by-one
5. Use pattern matching for simple if/else
6. Generate WASM module with `wasm-encoder`
7. Cache the result

This gives you a working system where callbacks defined in Rust/C can run in the browser without writing any JavaScript. The complexity is in the transpiler, but once built, it "just works" for simple cases.
