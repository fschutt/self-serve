use std::collections::HashMap;
use iced_x86::{Decoder, DecoderOptions, Instruction as X86Instr, Mnemonic, OpKind, Register};
use object::{Object, ObjectSection, ObjectSymbol};
use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, Function, FunctionSection,
    Instruction as WasmInstr, Module, TypeSection, ValType,
};

pub struct Transpiler {
    wasm_cache: HashMap<String, Vec<u8>>,
}

impl Transpiler {
    pub fn new() -> Self {
        let mut t = Transpiler {
            wasm_cache: HashMap::new(),
        };
        t.analyze_binary();
        t
    }

    fn analyze_binary(&mut self) {
        let binary_data = std::env::current_exe()
            .ok()
            .and_then(|p| std::fs::read(p).ok());

        for fn_name in ["increment_counter", "decrement_counter", "reset_counter"] {
            println!("Transpiling {}...", fn_name);

            let result = if let Some(ref data) = binary_data {
                self.try_from_binary(fn_name, data).or_else(|e| {
                    eprintln!("  Binary extraction failed ({}); using embedded x86-64 bytes", e);
                    self.from_fallback_bytes(fn_name)
                })
            } else {
                eprintln!("  Cannot read binary; using embedded x86-64 bytes");
                self.from_fallback_bytes(fn_name)
            };

            match result {
                Ok(wasm) => {
                    println!("  OK: {} bytes of WASM", wasm.len());
                    self.wasm_cache.insert(fn_name.to_string(), wasm);
                }
                Err(e) => eprintln!("  ERROR: {}", e),
            }
        }
    }

    fn try_from_binary(
        &self,
        fn_name: &str,
        data: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        // Only attempt if we're actually running as x86-64 — otherwise the binary
        // contains ARM64 (or other) machine code, not x86-64.
        if !cfg!(target_arch = "x86_64") {
            return Err("Not running as x86-64; native binary is a different architecture".into());
        }
        let (code, addr) = find_function_bytes(fn_name, data)?;
        let instrs = disassemble_until_ret(code, addr)?;
        println!(
            "  Decoded {} x86-64 instructions from binary:",
            instrs.len()
        );
        for i in &instrs {
            println!("    {:?}", i.mnemonic());
        }
        Ok(build_wasm_module(fn_name, &instrs))
    }

    fn from_fallback_bytes(
        &self,
        fn_name: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        // Pre-compiled x86-64 machine code for the three counter functions.
        // These are the exact bytes an optimising x86-64 compiler emits:
        //
        //   increment_counter:
        //     8D 47 01  lea eax, [rdi+1]   ; return value + 1
        //     C3        ret
        //
        //   decrement_counter:
        //     8D 47 FF  lea eax, [rdi-1]   ; return value - 1
        //     C3        ret
        //
        //   reset_counter:
        //     31 C0     xor eax, eax       ; return 0
        //     C3        ret
        let bytes: &[u8] = match fn_name {
            "increment_counter" => &[0x8D, 0x47, 0x01, 0xC3],
            "decrement_counter" => &[0x8D, 0x47, 0xFF, 0xC3],
            "reset_counter" => &[0x31, 0xC0, 0xC3],
            _ => return Err(format!("No fallback bytes for '{}'", fn_name).into()),
        };
        let instrs = disassemble_until_ret(bytes, 0)?;
        println!(
            "  Decoded {} x86-64 instructions from embedded bytes:",
            instrs.len()
        );
        for i in &instrs {
            println!("    {:?}", i.mnemonic());
        }
        Ok(build_wasm_module(fn_name, &instrs))
    }

    pub fn get_wasm_for_function(&self, fn_name: &str) -> Option<Vec<u8>> {
        self.wasm_cache.get(fn_name).cloned()
    }
}

// ─── Binary extraction ───────────────────────────────────────────────────────

fn find_function_bytes<'a>(
    fn_name: &str,
    data: &'a [u8],
) -> Result<(&'a [u8], u64), Box<dyn std::error::Error>> {
    let obj = object::File::parse(data)?;

    let mut found_addr = None;
    let mut found_size = 0u64;

    for sym in obj.symbols() {
        let name = sym.name().unwrap_or("");
        // Strip leading underscore (macOS Mach-O convention)
        let stripped = name.strip_prefix('_').unwrap_or(name);
        if stripped == fn_name {
            found_addr = Some(sym.address());
            found_size = sym.size();
            break;
        }
    }

    let addr = found_addr.ok_or_else(|| format!("Symbol '{}' not found in binary", fn_name))?;

    for section in obj.sections() {
        let sname = section.name().unwrap_or("");
        if sname == ".text" || sname == "__text" {
            let saddr = section.address();
            let sdata = section.data()?;
            if addr >= saddr && addr < saddr + sdata.len() as u64 {
                let off = (addr - saddr) as usize;
                let avail = sdata.len() - off;
                let len = if found_size > 0 && found_size as usize <= avail {
                    found_size as usize
                } else {
                    // Heuristic: read up to 512 bytes, stop at ret
                    avail.min(512)
                };
                return Ok((&sdata[off..off + len], addr));
            }
        }
    }

    Err(format!("Code for '{}' not found in .text section", fn_name).into())
}

// ─── Disassembly ─────────────────────────────────────────────────────────────

fn disassemble_until_ret(
    code: &[u8],
    rip: u64,
) -> Result<Vec<X86Instr>, Box<dyn std::error::Error>> {
    let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
    let mut instrs = Vec::new();
    while decoder.can_decode() {
        let instr = decoder.decode();
        let terminal = matches!(instr.mnemonic(), Mnemonic::Ret | Mnemonic::Retf);
        instrs.push(instr);
        if terminal {
            break;
        }
    }
    if instrs.is_empty() {
        return Err("No instructions decoded".into());
    }
    Ok(instrs)
}

// ─── Register allocator ──────────────────────────────────────────────────────

struct RegAlloc {
    regs: HashMap<Register, u32>,
    stack_slots: HashMap<i32, u32>, // RBP-relative offsets → local index
    next: u32,
}

impl RegAlloc {
    fn new() -> Self {
        let mut regs = HashMap::new();
        // Local 0 = parameter (RDI family), pre-allocated
        // Local 1 = return register (RAX family), pre-allocated
        regs.insert(norm(Register::RDI), 0);
        regs.insert(norm(Register::RAX), 1);
        Self {
            regs,
            stack_slots: HashMap::new(),
            next: 2,
        }
    }

    fn reg(&mut self, r: Register) -> Option<u32> {
        let n = norm(r);
        match n {
            Register::RSP | Register::RBP | Register::RIP | Register::None => None,
            _ => Some(
                *self
                    .regs
                    .entry(n)
                    .or_insert_with(|| {
                        let i = self.next;
                        self.next += 1;
                        i
                    }),
            ),
        }
    }

    fn stack_slot(&mut self, offset: i32) -> u32 {
        *self.stack_slots.entry(offset).or_insert_with(|| {
            let i = self.next;
            self.next += 1;
            i
        })
    }

    /// Number of additional locals beyond the single i32 parameter (local 0).
    fn extra_locals(&self) -> u32 {
        self.next.saturating_sub(1)
    }
}

fn norm(r: Register) -> Register {
    match r {
        Register::AL | Register::AH | Register::AX | Register::EAX => Register::RAX,
        Register::BL | Register::BH | Register::BX | Register::EBX => Register::RBX,
        Register::CL | Register::CH | Register::CX | Register::ECX => Register::RCX,
        Register::DL | Register::DH | Register::DX | Register::EDX => Register::RDX,
        Register::DIL | Register::DI | Register::EDI => Register::RDI,
        Register::SIL | Register::SI | Register::ESI => Register::RSI,
        Register::SPL | Register::SP | Register::ESP => Register::RSP,
        Register::BPL | Register::BP | Register::EBP => Register::RBP,
        Register::R8L | Register::R8D | Register::R8W => Register::R8,
        Register::R9L | Register::R9D | Register::R9W => Register::R9,
        Register::R10L | Register::R10D | Register::R10W => Register::R10,
        Register::R11L | Register::R11D | Register::R11W => Register::R11,
        Register::R12L | Register::R12D | Register::R12W => Register::R12,
        Register::R13L | Register::R13D | Register::R13W => Register::R13,
        Register::R14L | Register::R14D | Register::R14W => Register::R14,
        Register::R15L | Register::R15D | Register::R15W => Register::R15,
        _ => r,
    }
}

fn is_frame(r: Register) -> bool {
    matches!(norm(r), Register::RSP | Register::RBP)
}

/// Get the immediate of op1 as a signed i32.
fn imm_i32(instr: &X86Instr) -> i32 {
    match instr.op1_kind() {
        OpKind::Immediate8 => instr.immediate8() as i8 as i32,
        OpKind::Immediate8to32 => instr.immediate8() as i8 as i32,
        OpKind::Immediate16 => instr.immediate16() as i16 as i32,
        OpKind::Immediate32 => instr.immediate32() as i32,
        _ => 0,
    }
}

// ─── Translation ─────────────────────────────────────────────────────────────

fn translate(instrs: &[X86Instr]) -> (Vec<WasmInstr<'static>>, u32) {
    let mut alloc = RegAlloc::new();
    let mut ops: Vec<WasmInstr<'static>> = Vec::new();

    for instr in instrs {
        match instr.mnemonic() {
            // Frame management — skip entirely (WASM has no explicit stack frames)
            Mnemonic::Push | Mnemonic::Pop => {}

            Mnemonic::Mov => emit_mov(instr, &mut alloc, &mut ops),
            Mnemonic::Add => emit_add(instr, &mut alloc, &mut ops),
            Mnemonic::Sub => emit_sub(instr, &mut alloc, &mut ops),
            Mnemonic::Inc => emit_inc(instr, &mut alloc, &mut ops),
            Mnemonic::Dec => emit_dec(instr, &mut alloc, &mut ops),
            Mnemonic::Lea => emit_lea(instr, &mut alloc, &mut ops),
            Mnemonic::Xor => emit_xor(instr, &mut alloc, &mut ops),

            Mnemonic::Ret | Mnemonic::Retf => {
                // In x86-64 SysV ABI the return value is in EAX/RAX.
                if let Some(rax) = alloc.reg(Register::EAX) {
                    ops.push(WasmInstr::LocalGet(rax));
                    ops.push(WasmInstr::Return);
                }
            }

            // Silently ignore: NOP, ENDBR64, INT3, alignment padding, etc.
            _ => {}
        }
    }

    let extra = alloc.extra_locals();
    (ops, extra)
}

fn emit_mov(instr: &X86Instr, alloc: &mut RegAlloc, ops: &mut Vec<WasmInstr<'static>>) {
    match (instr.op0_kind(), instr.op1_kind()) {
        (OpKind::Register, OpKind::Register) => {
            let d = instr.op0_register();
            let s = instr.op1_register();
            if is_frame(d) || is_frame(s) {
                return;
            }
            if let (Some(src), Some(dst)) = (alloc.reg(s), alloc.reg(d)) {
                ops.push(WasmInstr::LocalGet(src));
                ops.push(WasmInstr::LocalSet(dst));
            }
        }
        (OpKind::Register, OpKind::Immediate32)
        | (OpKind::Register, OpKind::Immediate8to32)
        | (OpKind::Register, OpKind::Immediate8) => {
            let d = instr.op0_register();
            if is_frame(d) {
                return;
            }
            if let Some(dst) = alloc.reg(d) {
                ops.push(WasmInstr::I32Const(imm_i32(instr)));
                ops.push(WasmInstr::LocalSet(dst));
            }
        }
        // Store to stack: mov [rbp+off], reg
        (OpKind::Memory, OpKind::Register) if instr.memory_base() == Register::RBP => {
            let s = instr.op1_register();
            if let Some(src) = alloc.reg(s) {
                let slot = alloc.stack_slot(instr.memory_displacement32() as i32);
                ops.push(WasmInstr::LocalGet(src));
                ops.push(WasmInstr::LocalSet(slot));
            }
        }
        // Load from stack: mov reg, [rbp+off]
        (OpKind::Register, OpKind::Memory) if instr.memory_base() == Register::RBP => {
            let d = instr.op0_register();
            if is_frame(d) {
                return;
            }
            if let Some(dst) = alloc.reg(d) {
                let slot = alloc.stack_slot(instr.memory_displacement32() as i32);
                ops.push(WasmInstr::LocalGet(slot));
                ops.push(WasmInstr::LocalSet(dst));
            }
        }
        _ => {}
    }
}

fn emit_add(instr: &X86Instr, alloc: &mut RegAlloc, ops: &mut Vec<WasmInstr<'static>>) {
    if instr.op0_kind() != OpKind::Register {
        return;
    }
    let d = instr.op0_register();
    if is_frame(d) {
        return;
    }
    let Some(dst) = alloc.reg(d) else { return };
    match instr.op1_kind() {
        OpKind::Register => {
            if let Some(src) = alloc.reg(instr.op1_register()) {
                ops.push(WasmInstr::LocalGet(dst));
                ops.push(WasmInstr::LocalGet(src));
                ops.push(WasmInstr::I32Add);
                ops.push(WasmInstr::LocalSet(dst));
            }
        }
        OpKind::Immediate32 | OpKind::Immediate8to32 | OpKind::Immediate8 => {
            ops.push(WasmInstr::LocalGet(dst));
            ops.push(WasmInstr::I32Const(imm_i32(instr)));
            ops.push(WasmInstr::I32Add);
            ops.push(WasmInstr::LocalSet(dst));
        }
        _ => {}
    }
}

fn emit_sub(instr: &X86Instr, alloc: &mut RegAlloc, ops: &mut Vec<WasmInstr<'static>>) {
    if instr.op0_kind() != OpKind::Register {
        return;
    }
    let d = instr.op0_register();
    if is_frame(d) {
        return;
    }
    let Some(dst) = alloc.reg(d) else { return };
    match instr.op1_kind() {
        OpKind::Register => {
            if let Some(src) = alloc.reg(instr.op1_register()) {
                ops.push(WasmInstr::LocalGet(dst));
                ops.push(WasmInstr::LocalGet(src));
                ops.push(WasmInstr::I32Sub);
                ops.push(WasmInstr::LocalSet(dst));
            }
        }
        OpKind::Immediate32 | OpKind::Immediate8to32 | OpKind::Immediate8 => {
            ops.push(WasmInstr::LocalGet(dst));
            ops.push(WasmInstr::I32Const(imm_i32(instr)));
            ops.push(WasmInstr::I32Sub);
            ops.push(WasmInstr::LocalSet(dst));
        }
        _ => {}
    }
}

fn emit_inc(instr: &X86Instr, alloc: &mut RegAlloc, ops: &mut Vec<WasmInstr<'static>>) {
    if instr.op0_kind() != OpKind::Register {
        return;
    }
    if let Some(dst) = alloc.reg(instr.op0_register()) {
        ops.push(WasmInstr::LocalGet(dst));
        ops.push(WasmInstr::I32Const(1));
        ops.push(WasmInstr::I32Add);
        ops.push(WasmInstr::LocalSet(dst));
    }
}

fn emit_dec(instr: &X86Instr, alloc: &mut RegAlloc, ops: &mut Vec<WasmInstr<'static>>) {
    if instr.op0_kind() != OpKind::Register {
        return;
    }
    if let Some(dst) = alloc.reg(instr.op0_register()) {
        ops.push(WasmInstr::LocalGet(dst));
        ops.push(WasmInstr::I32Const(1));
        ops.push(WasmInstr::I32Sub);
        ops.push(WasmInstr::LocalSet(dst));
    }
}

fn emit_lea(instr: &X86Instr, alloc: &mut RegAlloc, ops: &mut Vec<WasmInstr<'static>>) {
    if instr.op0_kind() != OpKind::Register || instr.op1_kind() != OpKind::Memory {
        return;
    }
    let d = instr.op0_register();
    let base = instr.memory_base();
    // Signed displacement (iced-x86 sign-extends disp8/disp32)
    let disp = instr.memory_displacement32() as i32;
    let Some(dst) = alloc.reg(d) else { return };

    if base != Register::None && base != Register::RIP && !is_frame(base) {
        if let Some(b) = alloc.reg(base) {
            ops.push(WasmInstr::LocalGet(b));
            if disp != 0 {
                ops.push(WasmInstr::I32Const(disp));
                ops.push(WasmInstr::I32Add);
            }
            ops.push(WasmInstr::LocalSet(dst));
        }
    }
    // RIP-relative or RBP-relative LEA → skip (frame/address calculation we don't model)
}

fn emit_xor(instr: &X86Instr, alloc: &mut RegAlloc, ops: &mut Vec<WasmInstr<'static>>) {
    if instr.op0_kind() != OpKind::Register || instr.op1_kind() != OpKind::Register {
        return;
    }
    // xor reg, same_reg → zero
    if norm(instr.op0_register()) == norm(instr.op1_register()) {
        if let Some(dst) = alloc.reg(instr.op0_register()) {
            ops.push(WasmInstr::I32Const(0));
            ops.push(WasmInstr::LocalSet(dst));
        }
    }
}

// ─── WASM module builder ─────────────────────────────────────────────────────

fn build_wasm_module(fn_name: &str, instrs: &[X86Instr]) -> Vec<u8> {
    let (ops, extra_locals) = translate(instrs);

    let mut module = Module::new();

    // Type: (i32) -> i32
    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], [ValType::I32]);
    module.section(&types);

    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);

    let mut exports = ExportSection::new();
    exports.export(fn_name, ExportKind::Func, 0);
    module.section(&exports);

    let mut codes = CodeSection::new();
    let locals = if extra_locals > 0 {
        vec![(extra_locals, ValType::I32)]
    } else {
        vec![]
    };
    let mut func = Function::new(locals);
    for op in &ops {
        func.instruction(op);
    }
    func.instruction(&WasmInstr::End);
    codes.function(&func);
    module.section(&codes);

    module.finish()
}
