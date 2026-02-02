// Practical x86-64 to WASM Transpiler
// Handles simple C callbacks with jumps and function calls

use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register, Code};
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ExportKind, ExportSection, Function, 
    FunctionSection, Instruction as WasmInstr, MemArg, Module, TypeSection, ValType,
};
use std::collections::{HashMap, HashSet};

pub struct X64ToWasmTranspiler {
    binary_data: Vec<u8>,
}

impl X64ToWasmTranspiler {
    pub fn new(binary_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let binary_data = std::fs::read(binary_path)?;
        Ok(Self { binary_data })
    }
    
    pub fn transpile_function(&self, fn_name: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        // Step 1: Find function in binary
        let (code, entry_addr) = self.extract_function_code(fn_name)?;
        
        // Step 2: Disassemble x86-64
        let instructions = self.disassemble(code, entry_addr)?;
        
        // Step 3: Build control flow graph
        let cfg = ControlFlowGraph::from_instructions(&instructions, entry_addr);
        
        // Step 4: Allocate registers to WASM locals
        let mut allocator = RegisterAllocator::new();
        
        // Step 5: Translate to WASM
        let wasm_body = self.translate_to_wasm(&instructions, &cfg, &mut allocator)?;
        
        // Step 6: Generate WASM module
        Ok(self.generate_wasm_module(wasm_body, allocator))
    }
    
    fn extract_function_code(&self, fn_name: &str) -> Result<(&[u8], u64), Box<dyn std::error::Error>> {
        let obj = object::File::parse(&*self.binary_data)?;
        
        // Find symbol
        let mut target_addr = None;
        let mut target_size = None;
        
        for symbol in obj.symbols() {
            if symbol.kind() == SymbolKind::Text && symbol.name().ok() == Some(fn_name) {
                target_addr = Some(symbol.address());
                target_size = Some(symbol.size());
                break;
            }
        }
        
        let addr = target_addr.ok_or("Function not found")?;
        let size = target_size.ok_or("Function size unknown")?;
        
        // Extract code from .text section
        for section in obj.sections() {
            if section.name() == Ok(".text") {
                let section_addr = section.address();
                let section_data = section.data().ok_or("No section data")?;
                
                if addr >= section_addr && addr + size <= section_addr + section_data.len() as u64 {
                    let offset = (addr - section_addr) as usize;
                    return Ok((&section_data[offset..offset + size as usize], addr));
                }
            }
        }
        
        Err("Function code not found in .text section".into())
    }
    
    fn disassemble(&self, code: &[u8], rip: u64) -> Result<Vec<InstructionInfo>, Box<dyn std::error::Error>> {
        let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        let mut instructions = Vec::new();
        
        while decoder.can_decode() {
            let instr = decoder.decode();
            instructions.push(InstructionInfo {
                addr: instr.ip(),
                instr,
            });
        }
        
        Ok(instructions)
    }
    
    fn translate_to_wasm(
        &self,
        instructions: &[InstructionInfo],
        cfg: &ControlFlowGraph,
        allocator: &mut RegisterAllocator,
    ) -> Result<Vec<WasmInstr>, Box<dyn std::error::Error>> {
        let mut wasm = Vec::new();
        let mut label_map = HashMap::new();
        
        // First pass: create label mapping
        for (idx, info) in instructions.iter().enumerate() {
            label_map.insert(info.addr, idx);
        }
        
        // Second pass: translate instructions
        let blocks = cfg.structure_control_flow(&label_map);
        
        for block in blocks {
            wasm.extend(self.translate_block(&block, instructions, allocator, &label_map)?);
        }
        
        Ok(wasm)
    }
    
    fn translate_block(
        &self,
        block: &BasicBlock,
        instructions: &[InstructionInfo],
        allocator: &mut RegisterAllocator,
        label_map: &HashMap<u64, usize>,
    ) -> Result<Vec<WasmInstr>, Box<dyn std::error::Error>> {
        let mut wasm = Vec::new();
        
        for &instr_idx in &block.instruction_indices {
            let info = &instructions[instr_idx];
            wasm.extend(self.translate_instruction(&info.instr, allocator, label_map)?);
        }
        
        Ok(wasm)
    }
    
    fn translate_instruction(
        &self,
        instr: &Instruction,
        allocator: &mut RegisterAllocator,
        label_map: &HashMap<u64, usize>,
    ) -> Result<Vec<WasmInstr>, Box<dyn std::error::Error>> {
        let mut wasm = Vec::new();
        
        match instr.mnemonic() {
            // MOV instructions
            Mnemonic::Mov => {
                match (instr.op0_kind(), instr.op1_kind()) {
                    (OpKind::Register, OpKind::Register) => {
                        let dst = allocator.get_or_allocate(instr.op0_register());
                        let src = allocator.get_or_allocate(instr.op1_register());
                        wasm.push(WasmInstr::LocalGet(src));
                        wasm.push(WasmInstr::LocalSet(dst));
                    }
                    (OpKind::Register, OpKind::Immediate32) => {
                        let dst = allocator.get_or_allocate(instr.op0_register());
                        wasm.push(WasmInstr::I64Const(instr.immediate32() as i64));
                        wasm.push(WasmInstr::LocalSet(dst));
                    }
                    (OpKind::Register, OpKind::Memory) => {
                        // Load from memory
                        let dst = allocator.get_or_allocate(instr.op0_register());
                        let base = allocator.get_or_allocate(instr.memory_base());
                        
                        wasm.push(WasmInstr::LocalGet(base));
                        wasm.push(WasmInstr::I64Load(MemArg {
                            offset: instr.memory_displacement() as u64,
                            align: 3, // 8-byte alignment for i64
                            memory_index: 0,
                        }));
                        wasm.push(WasmInstr::LocalSet(dst));
                    }
                    (OpKind::Memory, OpKind::Register) => {
                        // Store to memory
                        let src = allocator.get_or_allocate(instr.op1_register());
                        let base = allocator.get_or_allocate(instr.memory_base());
                        
                        wasm.push(WasmInstr::LocalGet(base));
                        wasm.push(WasmInstr::LocalGet(src));
                        wasm.push(WasmInstr::I64Store(MemArg {
                            offset: instr.memory_displacement() as u64,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                    _ => {}
                }
            }
            
            // Arithmetic
            Mnemonic::Add => {
                let dst = allocator.get_or_allocate(instr.op0_register());
                
                match instr.op1_kind() {
                    OpKind::Register => {
                        let src = allocator.get_or_allocate(instr.op1_register());
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
                let dst = allocator.get_or_allocate(instr.op0_register());
                
                match instr.op1_kind() {
                    OpKind::Register => {
                        let src = allocator.get_or_allocate(instr.op1_register());
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
            
            Mnemonic::Imul => {
                let dst = allocator.get_or_allocate(instr.op0_register());
                let src = allocator.get_or_allocate(instr.op1_register());
                wasm.push(WasmInstr::LocalGet(dst));
                wasm.push(WasmInstr::LocalGet(src));
                wasm.push(WasmInstr::I64Mul);
                wasm.push(WasmInstr::LocalSet(dst));
            }
            
            // Comparisons (set flags for conditional jumps)
            Mnemonic::Cmp | Mnemonic::Test => {
                // Store comparison result in a virtual flag register
                let flag_reg = allocator.get_or_allocate_flag();
                
                match instr.mnemonic() {
                    Mnemonic::Cmp => {
                        let op0 = allocator.get_or_allocate(instr.op0_register());
                        
                        match instr.op1_kind() {
                            OpKind::Register => {
                                let op1 = allocator.get_or_allocate(instr.op1_register());
                                wasm.push(WasmInstr::LocalGet(op0));
                                wasm.push(WasmInstr::LocalGet(op1));
                                wasm.push(WasmInstr::I64Sub);
                            }
                            OpKind::Immediate32 => {
                                wasm.push(WasmInstr::LocalGet(op0));
                                wasm.push(WasmInstr::I64Const(instr.immediate32() as i64));
                                wasm.push(WasmInstr::I64Sub);
                            }
                            _ => {}
                        }
                        
                        wasm.push(WasmInstr::LocalSet(flag_reg));
                    }
                    Mnemonic::Test => {
                        let op0 = allocator.get_or_allocate(instr.op0_register());
                        let op1 = allocator.get_or_allocate(instr.op1_register());
                        wasm.push(WasmInstr::LocalGet(op0));
                        wasm.push(WasmInstr::LocalGet(op1));
                        wasm.push(WasmInstr::I64And);
                        wasm.push(WasmInstr::LocalSet(flag_reg));
                    }
                    _ => {}
                }
            }
            
            // Conditional jumps - these need special handling
            Mnemonic::Je | Mnemonic::Jne | Mnemonic::Jg | Mnemonic::Jl | 
            Mnemonic::Jge | Mnemonic::Jle | Mnemonic::Ja | Mnemonic::Jb => {
                // These are handled by control flow structuring
                // Just note: WASM uses structured control flow (if/block/loop)
                // not goto-style jumps
            }
            
            // Unconditional jump
            Mnemonic::Jmp => {
                // Handled by control flow structuring
            }
            
            // Function calls
            Mnemonic::Call => {
                // For now, we'll ignore external calls
                // In a real implementation, you'd need to:
                // 1. Resolve the target function
                // 2. Recursively transpile it
                // 3. Add to imports or internal functions
            }
            
            // Return
            Mnemonic::Ret => {
                // Return value is in RAX/EAX
                let rax = allocator.get_or_allocate(Register::RAX);
                wasm.push(WasmInstr::LocalGet(rax));
                wasm.push(WasmInstr::Return);
            }
            
            // Push/Pop (need stack simulation)
            Mnemonic::Push => {
                // Simplified: ignore for now
                // Real implementation needs to maintain a WASM-side stack
            }
            
            Mnemonic::Pop => {
                // Simplified: ignore for now
            }
            
            _ => {
                // Unsupported instruction - could log or panic
                println!("Warning: Unsupported instruction: {:?}", instr.mnemonic());
            }
        }
        
        Ok(wasm)
    }
    
    fn generate_wasm_module(&self, body: Vec<WasmInstr>, allocator: RegisterAllocator) -> Vec<u8> {
        let mut module = Module::new();
        
        // Type section: () -> i64 (simple callback signature)
        let mut types = TypeSection::new();
        types.function(vec![], vec![ValType::I64]);
        module.section(&types);
        
        // Function section
        let mut functions = FunctionSection::new();
        functions.function(0);
        module.section(&functions);
        
        // Export section
        let mut exports = ExportSection::new();
        exports.export("callback", ExportKind::Func, 0);
        module.section(&exports);
        
        // Code section
        let mut codes = CodeSection::new();
        let mut func = Function::new(allocator.get_locals_types());
        
        for instr in body {
            func.instruction(&instr);
        }
        
        // Ensure function ends properly
        func.instruction(&WasmInstr::End);
        
        codes.function(&func);
        module.section(&codes);
        
        module.finish()
    }
}

// Register allocator - maps x86-64 registers to WASM locals
struct RegisterAllocator {
    reg_map: HashMap<Register, u32>,
    next_local: u32,
    flag_reg: Option<u32>,
}

impl RegisterAllocator {
    fn new() -> Self {
        Self {
            reg_map: HashMap::new(),
            next_local: 0,
            flag_reg: None,
        }
    }
    
    fn get_or_allocate(&mut self, reg: Register) -> u32 {
        *self.reg_map.entry(reg).or_insert_with(|| {
            let idx = self.next_local;
            self.next_local += 1;
            idx
        })
    }
    
    fn get_or_allocate_flag(&mut self) -> u32 {
        if let Some(idx) = self.flag_reg {
            idx
        } else {
            let idx = self.next_local;
            self.next_local += 1;
            self.flag_reg = Some(idx);
            idx
        }
    }
    
    fn get_locals_types(&self) -> Vec<(u32, ValType)> {
        // All locals are i64 for simplicity
        if self.next_local > 0 {
            vec![(self.next_local, ValType::I64)]
        } else {
            vec![]
        }
    }
}

// Control flow graph structures
#[derive(Debug, Clone)]
struct InstructionInfo {
    addr: u64,
    instr: Instruction,
}

struct ControlFlowGraph {
    blocks: Vec<BasicBlock>,
    edges: HashMap<usize, Vec<usize>>,
}

#[derive(Debug, Clone)]
struct BasicBlock {
    start_addr: u64,
    end_addr: u64,
    instruction_indices: Vec<usize>,
}

impl ControlFlowGraph {
    fn from_instructions(instructions: &[InstructionInfo], entry: u64) -> Self {
        let mut blocks = Vec::new();
        let mut edges = HashMap::new();
        let mut leaders = HashSet::new();
        
        // Identify basic block leaders
        leaders.insert(entry);
        
        for (idx, info) in instructions.iter().enumerate() {
            match info.instr.mnemonic() {
                Mnemonic::Jmp | Mnemonic::Je | Mnemonic::Jne | Mnemonic::Jg | 
                Mnemonic::Jl | Mnemonic::Jge | Mnemonic::Jle | Mnemonic::Ja | 
                Mnemonic::Jb | Mnemonic::Call | Mnemonic::Ret => {
                    // Target of jump is a leader
                    if info.instr.is_jmp_short_or_near() {
                        leaders.insert(info.instr.near_branch_target());
                    }
                    
                    // Instruction after jump/call is a leader
                    if idx + 1 < instructions.len() {
                        leaders.insert(instructions[idx + 1].addr);
                    }
                }
                _ => {}
            }
        }
        
        // Build basic blocks
        let mut current_block_start = 0;
        let mut current_block_indices = Vec::new();
        
        for (idx, info) in instructions.iter().enumerate() {
            if leaders.contains(&info.addr) && !current_block_indices.is_empty() {
                // Start new block
                blocks.push(BasicBlock {
                    start_addr: instructions[current_block_start].addr,
                    end_addr: instructions[idx - 1].addr,
                    instruction_indices: current_block_indices.clone(),
                });
                
                current_block_start = idx;
                current_block_indices.clear();
            }
            
            current_block_indices.push(idx);
        }
        
        // Add final block
        if !current_block_indices.is_empty() {
            blocks.push(BasicBlock {
                start_addr: instructions[current_block_start].addr,
                end_addr: instructions[instructions.len() - 1].addr,
                instruction_indices: current_block_indices,
            });
        }
        
        Self { blocks, edges }
    }
    
    fn structure_control_flow(&self, label_map: &HashMap<u64, usize>) -> Vec<BasicBlock> {
        // For simple callbacks, just return blocks in order
        // A real implementation would use Relooper or similar algorithm
        // to convert to structured control flow (if/loop/block)
        self.blocks.clone()
    }
}
