/// Full pipeline trace for increment_counter:
///   x86-64 bytes → iced-x86 decode → register allocation → wasm-encoder → WASM binary
use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpKind, Register};
use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, Function, FunctionSection,
    Instruction as WasmInstr, Module, TypeSection, ValType,
};

fn main() {
    // ── 1. SOURCE CODE ──────────────────────────────────────────────────────
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  STEP 1 — Rust source function                               ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("  #[no_mangle]");
    println!("  #[inline(never)]");
    println!("  pub extern \"C\" fn increment_counter(value: i32) -> i32 {{");
    println!("      value + 1");
    println!("  }}");
    println!();
    println!("  Calling convention: System V AMD64 ABI");
    println!("    • first i32 argument  → EDI  (lower 32 bits of RDI)");
    println!("    • i32 return value    → EAX  (lower 32 bits of RAX)");

    // ── 2. X86-64 MACHINE CODE ──────────────────────────────────────────────
    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  STEP 2 — x86-64 machine code bytes                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // These are the exact bytes an optimising x86-64 compiler emits for
    // `fn increment_counter(value: i32) -> i32 { value + 1 }`.
    //
    // On x86-64 the server reads these from its OWN .text section at runtime
    // (using the `object` crate + dlsym-style symbol lookup).
    // On ARM64 (this machine) they are pre-embedded constants.
    let bytes: &[u8] = &[
        0x8D, 0x47, 0x01, // lea eax, [rdi+1]
        0xC3,             // ret
    ];

    print!("  Bytes: ");
    for b in bytes {
        print!("{:02X} ", b);
    }
    println!();
    println!();
    println!("  Byte-by-byte encoding:");
    println!("    8D          — opcode: LEA r32, m  (Load Effective Address, 32-bit dst)");
    println!("    47          — ModRM byte: 0 1 0 0 0 1 1 1");
    println!("                    mod=01  → 8-bit signed displacement follows");
    println!("                    reg=000 → destination register: EAX (register 0)");
    println!("                    r/m=111 → base register: RDI (register 7)");
    println!("    01          — disp8: +1  (the displacement, sign-extended to 64-bit)");
    println!("    C3          — opcode: RET (near return)");

    // ── 3. DISASSEMBLY ──────────────────────────────────────────────────────
    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  STEP 3 — iced-x86 decodes the bytes                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    let mut decoder = Decoder::with_ip(64, bytes, 0x0000, DecoderOptions::NONE);
    let mut instructions = Vec::new();
    while decoder.can_decode() {
        let instr = decoder.decode();
        let is_ret = matches!(instr.mnemonic(), Mnemonic::Ret | Mnemonic::Retf);
        instructions.push(instr.clone());
        if is_ret { break; }
    }

    for instr in &instructions {
        let start = instr.ip() as usize;
        let end   = start + instr.len();
        let raw: Vec<String> = bytes[start..end].iter().map(|b| format!("{:02X}", b)).collect();
        println!(
            "  [{:04x}]  {:<12}  mnemonic={:?}",
            instr.ip(), raw.join(" "), instr.mnemonic()
        );
        if instr.op_count() > 0 {
            for i in 0..instr.op_count() {
                let kind = match i {
                    0 => instr.op0_kind(),
                    1 => instr.op1_kind(),
                    _ => unreachable!(),
                };
                println!("             op{}  kind={:?}", i, kind);
                match kind {
                    OpKind::Register => {
                        let reg = if i == 0 { instr.op0_register() } else { instr.op1_register() };
                        println!("                  register={:?}", reg);
                    }
                    OpKind::Memory => {
                        println!("                  memory_base={:?}", instr.memory_base());
                        println!(
                            "                  displacement=0x{:08X} = {}  (sign-ext from disp8 0x{:02X})",
                            instr.memory_displacement32(),
                            instr.memory_displacement32() as i32,
                            bytes[start + instr.len() - 1]  // the disp byte
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    // ── 4. REGISTER ALLOCATION ──────────────────────────────────────────────
    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  STEP 4 — register allocator maps x86-64 regs → WASM locals  ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("  WASM functions use numbered locals, not named registers.");
    println!("  Parameters count as local 0, 1, … (already allocated).");
    println!();
    println!("  Pre-allocation (always done at startup):");
    println!("    RDI family (EDI, DI, DIL, RDI)  → local 0  [= the i32 parameter]");
    println!("    RAX family (EAX, AX, AL, RAX)   → local 1  [= the i32 return slot]");
    println!("    next_local counter starts at 2");
    println!();
    println!("  No additional registers appear in this function,");
    println!("  so total locals declared = 1 extra (local 1, RAX).");
    println!("  Local 0 is the parameter — not re-declared in the locals section.");

    // ── 5. INSTRUCTION TRANSLATION ──────────────────────────────────────────
    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  STEP 5 — translate each x86-64 instruction to WASM ops      ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    println!("  x86:  lea  eax, [rdi + 1]");
    println!("  ─────────────────────────");
    println!("  Pattern: Lea, op0=Register(EAX), op1=Memory(base=RDI, disp=+1)");
    println!("  → norm(RDI) = local 0  (the input parameter)");
    println!("  → norm(EAX) = local 1  (the return-value slot)");
    println!("  → disp ≠ 0, so emit add");
    println!("  Emits:");
    println!("    local.get 0    ; push local[0]  = the i32 argument  (RDI)");
    println!("    i32.const 1    ; push the displacement +1");
    println!("    i32.add        ; pop two, push their sum");
    println!("    local.set 1    ; pop sum → local[1]  (EAX return slot)");
    println!();
    println!("  x86:  ret");
    println!("  ─────────");
    println!("  Pattern: Ret");
    println!("  → SysV ABI return value is in EAX → norm(EAX) = local 1");
    println!("  Emits:");
    println!("    local.get 1    ; push local[1]  (the computed value)");
    println!("    return         ; exit function, leaving that value on the stack");
    println!();
    println!("  Final instruction list:");
    let wasm_ops = [
        ("local.get 0", "push argument (RDI)"),
        ("i32.const 1", "push displacement +1"),
        ("i32.add    ", "pop & add → RDI+1"),
        ("local.set 1", "store result in EAX slot"),
        ("local.get 1", "push EAX slot (return value)"),
        ("return     ", "exit function"),
        ("end        ", "close function body"),
    ];
    for (op, comment) in &wasm_ops {
        println!("    {}   ; {}", op, comment);
    }

    // ── 6. WASM BINARY ENCODING ─────────────────────────────────────────────
    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  STEP 6 — wasm-encoder assembles the binary                  ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Build it for real and annotate
    let mut module = Module::new();

    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], [ValType::I32]);
    module.section(&types);

    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);

    let mut exports = ExportSection::new();
    exports.export("increment_counter", ExportKind::Func, 0);
    module.section(&exports);

    let mut codes = CodeSection::new();
    let mut func = Function::new(vec![(1u32, ValType::I32)]); // 1 extra local (EAX)
    func.instruction(&WasmInstr::LocalGet(0));
    func.instruction(&WasmInstr::I32Const(1));
    func.instruction(&WasmInstr::I32Add);
    func.instruction(&WasmInstr::LocalSet(1));
    func.instruction(&WasmInstr::LocalGet(1));
    func.instruction(&WasmInstr::Return);
    func.instruction(&WasmInstr::End);
    codes.function(&func);
    module.section(&codes);

    let wasm = module.finish();

    println!("  The module is split into sections, each with a 1-byte section id.");
    println!();

    // Walk the bytes and annotate them
    annotate_wasm(&wasm);

    // ── 7. VALIDATION ───────────────────────────────────────────────────────
    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  STEP 7 — produced binary (hex dump)                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    print!("  ");
    for (i, b) in wasm.iter().enumerate() {
        if i > 0 && i % 16 == 0 { print!("\n  "); }
        print!("{:02X} ", b);
    }
    println!();
    println!();
    println!("  {} bytes total", wasm.len());
    println!();
    println!("  This is what the server caches and serves at /wasm/increment_counter.");
    println!("  The browser fetches it, calls WebAssembly.instantiate(), and runs it.");
}

fn annotate_wasm(wasm: &[u8]) {
    // Sequential walker — show!(n, label) consumes n bytes and advances the cursor.
    let mut i = 0usize;
    macro_rules! show {
        ($n:expr, $label:expr) => {{
            let n: usize = $n;
            let hex: Vec<String> = wasm[i..i+n].iter().map(|b| format!("{:02X}",b)).collect();
            println!("  [{:02X}..{:02X}]  {:<28}  {}",
                     i, i+n-1, hex.join(" "), $label);
            i += n;
        }};
    }

    // ── header ──
    show!(4, "magic: \\0asm");
    show!(4, "version: 1");

    // ── type section ──
    show!(1, "section id=1  (Type)");
    let (tlen, tlen_w) = read_leb128_u32(&wasm[i..]); show!(tlen_w, format!("  length: {} B", tlen));
    show!(1, "  type count: 1");
    show!(1, "  type[0] marker: 0x60 = func");
    show!(1, "    param count: 1");
    show!(1, "    param[0]: 0x7F = i32");
    show!(1, "    result count: 1");
    show!(1, "    result[0]: 0x7F = i32");

    // ── function section ──
    show!(1, "section id=3  (Function)");
    let (flen, flen_w) = read_leb128_u32(&wasm[i..]); show!(flen_w, format!("  length: {} B", flen));
    show!(1, "  func count: 1");
    show!(1, "  func[0]: uses type[0]");

    // ── export section ──
    show!(1, "section id=7  (Export)");
    let (elen, elen_w) = read_leb128_u32(&wasm[i..]); show!(elen_w, format!("  length: {} B", elen));
    show!(1, "  export count: 1");
    let (nlen, nlen_w) = read_leb128_u32(&wasm[i..]); show!(nlen_w, format!("  name length: {}", nlen));
    let name = std::str::from_utf8(&wasm[i..i+nlen as usize]).unwrap().to_string();
    show!(nlen as usize, format!("  name: \"{}\"", name));
    show!(1, "  kind: 0x00 = function");
    show!(1, "  index: 0  (func[0])");

    // ── code section ──
    show!(1, "section id=10 (Code)");
    let (clen, clen_w) = read_leb128_u32(&wasm[i..]); show!(clen_w, format!("  length: {} B", clen));
    show!(1, "  body count: 1");
    let (blen, blen_w) = read_leb128_u32(&wasm[i..]); show!(blen_w, format!("  body[0] size: {} B", blen));
    show!(1, "    local decl count: 1");
    show!(1, "      run length: 1  (one i32 local = the EAX slot)");
    show!(1, "      type: 0x7F = i32");

    println!("    [instructions]");
    while i < wasm.len() {
        let op = wasm[i];
        match op {
            0x20 => { let idx = wasm[i+1]; show!(2, format!("      local.get {}  ; push local[{}]", idx, idx)); }
            0x21 => { let idx = wasm[i+1]; show!(2, format!("      local.set {}  ; pop → local[{}]", idx, idx)); }
            0x41 => {
                let (val, nb) = read_leb128_i32(&wasm[i+1..]);
                show!(1+nb, format!("      i32.const {}  ; push literal", val));
            }
            0x6A => { show!(1, "      i32.add       ; pop 2, push sum"); }
            0x6B => { show!(1, "      i32.sub       ; pop 2, push difference"); }
            0x0F => { show!(1, "      return        ; exit, top-of-stack is result"); }
            0x0B => { show!(1, "      end           ; close function body"); break; }
            _ => { show!(1, format!("      0x{:02X}", op)); }
        }
    }
}

fn read_leb128_i32(bytes: &[u8]) -> (i32, usize) {
    let mut result = 0i32;
    let mut shift = 0;
    let mut n = 0;
    loop {
        let byte = bytes[n]; n += 1;
        result |= ((byte & 0x7F) as i32) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 32 && (byte & 0x40) != 0 { result |= !0i32 << shift; }
            break;
        }
    }
    (result, n)
}

fn read_leb128_u32(bytes: &[u8]) -> (u32, usize) {
    let mut result = 0u32;
    let mut shift = 0;
    let mut n = 0;
    loop {
        let byte = bytes[n]; n += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        shift += 7;
        if byte & 0x80 == 0 { break; }
    }
    (result, n)
}
