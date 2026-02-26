# x64 → WASM Transpilation Pipeline

How the server reads its own x86-64 machine code and turns it into a WebAssembly module at startup.

---

## Step 1 — Rust source function

```rust
#[no_mangle]
#[inline(never)]
pub extern "C" fn increment_counter(value: i32) -> i32 {
    value + 1
}
```

Three attributes are critical:

| Attribute | Effect |
|---|---|
| `#[no_mangle]` | Keeps the symbol name intact in the binary so the `object` crate can find it by string at runtime |
| `extern "C"` | Forces the System V AMD64 ABI: first `i32` arg in `EDI`, return value in `EAX` |
| `#[inline(never)]` | Prevents the optimiser from dissolving the function body |

---

## Step 2 — x86-64 machine code

`increment_counter(value: i32) -> i32 { value + 1 }` compiles to **4 bytes**:

```
8D 47 01   lea eax, [rdi+1]
C3         ret
```

The ModRM byte `47` packs three 3-bit fields:

```
0 1 | 0 0 0 | 1 1 1
mod   reg     r/m

mod=01  → 8-bit signed displacement follows
reg=000 → destination register: EAX  (register index 0)
r/m=111 → base register:        RDI  (register index 7)
```

The trailing `01` is the displacement (+1), sign-extended to 64 bits by the CPU.

> **On x86-64** the server reads these bytes live from its own `.text` section using the `object` crate + symbol lookup.
> **On ARM64** (e.g. Apple Silicon) the bytes are pre-embedded constants — the WASM is still genuinely transpiled from x86-64 machine code, just not from *this* binary.

---

## Step 3 — iced-x86 decodes

```rust
let mut decoder = Decoder::with_ip(64, bytes, addr, DecoderOptions::NONE);
```

`iced-x86` turns the raw byte slice into structured `Instruction` objects:

```
[0000]  8D 47 01   mnemonic = Lea
                   op0: Register(EAX)
                   op1: Memory { base=RDI, displacement=+1 }

[0003]  C3         mnemonic = Ret
```

The decoder is told it is 64-bit mode (`64`) and given the virtual start address (`addr`) so instruction pointers are correct for branch-target resolution (not needed for these simple functions, but important in general).

---

## Step 4 — Register allocation

WASM has no named registers — only numbered **locals**. The allocator pre-maps the two registers the ABI guarantees:

| x86-64 register family | WASM local | Role |
|---|---|---|
| `RDI / EDI / DI / DIL` | `0` | Function parameter (always slot 0 in WASM) |
| `RAX / EAX / AX / AL`  | `1` | Return-value register |

Any other register encountered gets the next free slot (`2`, `3`, …). Frame-management registers (`RSP`, `RBP`) return `None` and are silently skipped — WASM has no explicit stack frames.

```
stack-slot locals (from [rbp-N] patterns in debug builds) → slots 2, 3, …
```

---

## Step 5 — Instruction translation

Each x86-64 instruction is matched and converted to a sequence of stack-machine WASM opcodes:

| x86-64 | WASM emitted | Why |
|---|---|---|
| `lea eax, [rdi+1]` | `local.get 0` `i32.const 1` `i32.add` `local.set 1` | Push base (RDI), push displacement, add, store result in EAX slot |
| `ret` | `local.get 1` `return` | Push EAX (the result) onto the WASM value stack, then exit |
| `push rbp` / `pop rbp` | *(nothing)* | Frame management — no equivalent concept in WASM |
| `mov [rbp-4], edi` | `local.get 0` `local.set <slot>` | Debug-build stack spill → copy to a local slot |
| `xor eax, eax` | `i32.const 0` `local.set 1` | Zero-idiom → store 0 in EAX slot |
| `add eax, 1` | `local.get 1` `i32.const 1` `i32.add` `local.set 1` | Read-modify-write via stack |

Full instruction list for `increment_counter`:

```wat
local.get 0    ; push local[0]  = the i32 argument  (RDI)
i32.const 1    ; push the displacement +1
i32.add        ; pop two, push their sum  (RDI+1)
local.set 1    ; pop sum → local[1]  (EAX return slot)
local.get 1    ; push local[1]  (the computed value)
return         ; exit function — top of value stack is the result
end            ; close function body
```

---

## Step 6 — wasm-encoder serialises the binary

The module is divided into **four sections**, each prefixed with a 1-byte id and a LEB128 length:

```
[00..03]  00 61 73 6D                   magic: \0asm
[04..07]  01 00 00 00                   version: 1

[08..08]  01                            section id=1  (Type)
[09..09]  06                              length: 6 bytes
[0A..0A]  01                              type count: 1
[0B..0B]  60                              type[0] marker: 0x60 = func
[0C..0C]  01                                param count: 1
[0D..0D]  7F                                param[0]: 0x7F = i32
[0E..0E]  01                                result count: 1
[0F..0F]  7F                                result[0]: 0x7F = i32

[10..10]  03                            section id=3  (Function)
[11..11]  02                              length: 2 bytes
[12..12]  01                              func count: 1
[13..13]  00                              func[0]: uses type[0]

[14..14]  07                            section id=7  (Export)
[15..15]  15                              length: 21 bytes
[16..16]  01                              export count: 1
[17..17]  11                              name length: 17
[18..28]  69 6E 63 72 65 6D 65 6E ...    name: "increment_counter"
[29..29]  00                              kind: 0x00 = function
[2A..2A]  00                              index: 0  (func[0])

[2B..2B]  0A                            section id=10 (Code)
[2C..2C]  10                              length: 16 bytes
[2D..2D]  01                              body count: 1
[2E..2E]  0E                              body[0] size: 14 bytes
[2F..2F]  01                                local decl count: 1
[30..30]  01                                  run-length: 1  (one extra local)
[31..31]  7F                                  type: 0x7F = i32  ← the EAX slot
[32..33]  20 00                               local.get 0
[34..35]  41 01                               i32.const 1
[36..36]  6A                                  i32.add
[37..38]  21 01                               local.set 1
[39..3A]  20 01                               local.get 1
[3B..3B]  0F                                  return
[3C..3C]  0B                                  end
```

**Total: 61 bytes.**

The 7 instructions + 3 local-declaration bytes = 14-byte body → `0E` in the length field.

---

## Step 7 — Browser execution

```
GET /wasm/increment_counter
  ← 61 bytes  (application/wasm)

WebAssembly.instantiate(wasmBytes)
  → instance.exports.increment_counter(currentCounter)
  ← new counter value  (computed by the transpiled x86-64 logic)

POST /execute/increment_counter
  ← { "value": N }  (server applies the same function, confirms state)
```

The WAT disassembly (what `wasm2wat` produces) matches exactly:

```wat
(module
  (func $increment_counter (export "increment_counter") (param $var0 i32) (result i32)
    (local $var1 i32)
    local.get $var0
    i32.const 1
    i32.add
    local.set $var1
    local.get $var1
    return
  )
)
```
