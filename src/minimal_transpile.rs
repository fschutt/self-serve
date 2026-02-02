// Minimal Working Example: Complete x64 to WASM Transpiler
// This shows the full pipeline for a simple callback function

use std::collections::HashMap;

// Example: Transpile this simple C callback
// 
// int add_one(int x) {
//     return x + 1;
// }
//
// Assembly:
// add_one:
//     lea eax, [rdi+1]    ; or: add rdi, 1; mov eax, edi
//     ret

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Get function bytes from your binary
    // In real code, use object crate to extract from ELF
    
    // For this example, we'll use pre-disassembled bytes
    // x86-64 for: add_one(int x) -> int
    let function_bytes = vec![
        0x8d, 0x47, 0x01,  // lea eax, [rdi+1]
        0xc3,              // ret
    ];
    
    // Step 2: Transpile
    let transpiler = SimpleTranspiler::new();
    let wasm_bytes = transpiler.transpile(&function_bytes, 0x1000)?;
    
    // Step 3: Write to file
    std::fs::write("add_one.wasm", &wasm_bytes)?;
    
    println!("Successfully transpiled! Output: add_one.wasm");
    println!("WASM size: {} bytes", wasm_bytes.len());
    
    Ok(())
}

struct SimpleTranspiler;

impl SimpleTranspiler {
    fn new() -> Self {
        Self
    }
    
    fn transpile(&self, code: &[u8], start_addr: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        use iced_x86::{Decoder, DecoderOptions, Instruction};
        
        // Disassemble
        let mut decoder = Decoder::with_ip(64, code, start_addr, DecoderOptions::NONE);
        let mut instructions = Vec::new();
        
        while decoder.can_decode() {
            instructions.push(decoder.decode());
        }
        
        println!("Disassembled {} instructions:", instructions.len());
        for instr in &instructions {
            println!("  {:?}", instr);
        }
        
        // Translate
        let mut translator = WasmTranslator::new();
        let wasm_instructions = translator.translate(&instructions)?;
        
        // Generate WASM module
        let wasm_bytes = self.generate_module(wasm_instructions, translator.num_locals());
        
        Ok(wasm_bytes)
    }
    
    fn generate_module(&self, instructions: Vec<WasmInstr>, num_locals: u32) -> Vec<u8> {
        use wasm_encoder::*;
        
        let mut module = Module::new();
        
        // Type section: (i64) -> i64
        let mut types = TypeSection::new();
        types.function(vec![ValType::I64], vec![ValType::I64]);
        module.section(&types);
        
        // Function section
        let mut functions = FunctionSection::new();
        functions.function(0); // Use type 0
        module.section(&functions);
        
        // Memory section (needed for loads/stores)
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: 1,
            maximum: Some(1),
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);
        
        // Export section
        let mut exports = ExportSection::new();
        exports.export("callback", ExportKind::Func, 0);
        exports.export("memory", ExportKind::Memory, 0);
        module.section(&exports);
        
        // Code section
        let mut codes = CodeSection::new();
        let mut func = Function::new(vec![(num_locals, ValType::I64)]);
        
        for instr in instructions {
            func.instruction(&instr);
        }
        
        func.instruction(&Instruction::End);
        codes.function(&func);
        module.section(&codes);
        
        module.finish()
    }
}

#[derive(Debug, Clone, Copy)]
enum WasmInstr {
    LocalGet(u32),
    LocalSet(u32),
    I64Const(i64),
    I64Add,
    I64Sub,
    I64Load { offset: u32, align: u32 },
    I64Store { offset: u32, align: u32 },
    Return,
    End,
}

struct WasmTranslator {
    registers: HashMap<iced_x86::Register, u32>,
    next_local: u32,
}

impl WasmTranslator {
    fn new() -> Self {
        Self {
            registers: HashMap::new(),
            next_local: 1, // Local 0 is the parameter
        }
    }
    
    fn num_locals(&self) -> u32 {
        self.next_local
    }
    
    fn get_or_allocate_register(&mut self, reg: iced_x86::Register) -> u32 {
        use iced_x86::Register;
        
        // Map register to local
        // For parameters, use special mapping
        match reg {
            Register::RDI | Register::EDI => 0, // First parameter
            _ => {
                if let Some(&local) = self.registers.get(&reg) {
                    local
                } else {
                    let local = self.next_local;
                    self.next_local += 1;
                    self.registers.insert(reg, local);
                    local
                }
            }
        }
    }
    
    fn translate(&mut self, instructions: &[iced_x86::Instruction]) -> Result<Vec<WasmInstr>, Box<dyn std::error::Error>> {
        use iced_x86::{Mnemonic, OpKind};
        
        let mut wasm = Vec::new();
        
        for instr in instructions {
            println!("Translating: {:?}", instr.mnemonic());
            
            match instr.mnemonic() {
                Mnemonic::Lea => {
                    // lea eax, [rdi+1] -> local.set $eax (i64.add (local.get $rdi) (i64.const 1))
                    let dst = self.get_or_allocate_register(instr.op0_register());
                    let base = self.get_or_allocate_register(instr.memory_base());
                    let offset = instr.memory_displacement() as i64;
                    
                    wasm.push(WasmInstr::LocalGet(base));
                    wasm.push(WasmInstr::I64Const(offset));
                    wasm.push(WasmInstr::I64Add);
                    wasm.push(WasmInstr::LocalSet(dst));
                }
                
                Mnemonic::Mov => {
                    match (instr.op0_kind(), instr.op1_kind()) {
                        (OpKind::Register, OpKind::Register) => {
                            let dst = self.get_or_allocate_register(instr.op0_register());
                            let src = self.get_or_allocate_register(instr.op1_register());
                            wasm.push(WasmInstr::LocalGet(src));
                            wasm.push(WasmInstr::LocalSet(dst));
                        }
                        (OpKind::Register, OpKind::Immediate32) => {
                            let dst = self.get_or_allocate_register(instr.op0_register());
                            wasm.push(WasmInstr::I64Const(instr.immediate32() as i64));
                            wasm.push(WasmInstr::LocalSet(dst));
                        }
                        (OpKind::Register, OpKind::Memory) => {
                            let dst = self.get_or_allocate_register(instr.op0_register());
                            let base = self.get_or_allocate_register(instr.memory_base());
                            let offset = instr.memory_displacement() as u32;
                            
                            wasm.push(WasmInstr::LocalGet(base));
                            wasm.push(WasmInstr::I64Load { offset, align: 3 });
                            wasm.push(WasmInstr::LocalSet(dst));
                        }
                        _ => {}
                    }
                }
                
                Mnemonic::Add => {
                    let dst = self.get_or_allocate_register(instr.op0_register());
                    
                    match instr.op1_kind() {
                        OpKind::Register => {
                            let src = self.get_or_allocate_register(instr.op1_register());
                            wasm.push(WasmInstr::LocalGet(dst));
                            wasm.push(WasmInstr::LocalGet(src));
                            wasm.push(WasmInstr::I64Add);
                            wasm.push(WasmInstr::LocalSet(dst));
                        }
                        OpKind::Immediate32 => {
                            wasm.push(WasmInstr::LocalGet(dst));
                            wasm.push(WasmInstr::I64Const(instr.immediate32() as i64));
                            wasm.push(WasmInstr::I64Add);
                            wasm.push(WasmInstr::LocalSet(dst));
                        }
                        _ => {}
                    }
                }
                
                Mnemonic::Sub => {
                    let dst = self.get_or_allocate_register(instr.op0_register());
                    
                    match instr.op1_kind() {
                        OpKind::Register => {
                            let src = self.get_or_allocate_register(instr.op1_register());
                            wasm.push(WasmInstr::LocalGet(dst));
                            wasm.push(WasmInstr::LocalGet(src));
                            wasm.push(WasmInstr::I64Sub);
                            wasm.push(WasmInstr::LocalSet(dst));
                        }
                        OpKind::Immediate32 => {
                            wasm.push(WasmInstr::LocalGet(dst));
                            wasm.push(WasmInstr::I64Const(instr.immediate32() as i64));
                            wasm.push(WasmInstr::I64Sub);
                            wasm.push(WasmInstr::LocalSet(dst));
                        }
                        _ => {}
                    }
                }
                
                Mnemonic::Ret => {
                    // Return value is in RAX/EAX
                    use iced_x86::Register;
                    let rax = self.get_or_allocate_register(Register::RAX);
                    wasm.push(WasmInstr::LocalGet(rax));
                    wasm.push(WasmInstr::Return);
                }
                
                _ => {
                    println!("Warning: Unsupported instruction {:?}", instr.mnemonic());
                }
            }
        }
        
        Ok(wasm)
    }
}

// Convert our simple WasmInstr to wasm_encoder::Instruction
impl From<WasmInstr> for wasm_encoder::Instruction<'static> {
    fn from(instr: WasmInstr) -> Self {
        use wasm_encoder::Instruction;
        
        match instr {
            WasmInstr::LocalGet(idx) => Instruction::LocalGet(idx),
            WasmInstr::LocalSet(idx) => Instruction::LocalSet(idx),
            WasmInstr::I64Const(val) => Instruction::I64Const(val),
            WasmInstr::I64Add => Instruction::I64Add,
            WasmInstr::I64Sub => Instruction::I64Sub,
            WasmInstr::I64Load { offset, align } => {
                Instruction::I64Load(wasm_encoder::MemArg {
                    offset: offset as u64,
                    align,
                    memory_index: 0,
                })
            }
            WasmInstr::I64Store { offset, align } => {
                Instruction::I64Store(wasm_encoder::MemArg {
                    offset: offset as u64,
                    align,
                    memory_index: 0,
                })
            }
            WasmInstr::Return => Instruction::Return,
            WasmInstr::End => Instruction::End,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_simple_add() {
        // mov eax, edi
        // add eax, 1
        // ret
        let code = vec![
            0x89, 0xf8,        // mov eax, edi
            0x83, 0xc0, 0x01,  // add eax, 1
            0xc3,              // ret
        ];
        
        let transpiler = SimpleTranspiler::new();
        let wasm = transpiler.transpile(&code, 0x1000).unwrap();
        
        assert!(!wasm.is_empty());
        assert!(wasm.starts_with(b"\0asm")); // WASM magic number
    }
}
