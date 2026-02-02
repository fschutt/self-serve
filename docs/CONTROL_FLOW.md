# Handling Jumps, Labels, and Control Flow

The trickiest part of x86-64 to WASM transpilation is converting unstructured jumps to WASM's structured control flow.

## The Problem

x86-64 has unstructured control flow:
```asm
cmp rax, 0
je .label_zero      ; Jump to label if equal
add rax, 1
jmp .label_done     ; Unconditional jump
.label_zero:
mov rax, 0
.label_done:
ret
```

WASM has structured control flow:
```wasm
(if (i64.eqz (local.get $rax))
  (then
    (local.set $rax (i64.const 0))
  )
  (else
    (local.set $rax (i64.add (local.get $rax) (i64.const 1)))
  )
)
```

## Strategy: Relooper Algorithm

For simple callbacks, a simplified approach works:

### Step 1: Identify Basic Blocks

A basic block is a sequence of instructions with:
- One entry point (beginning)
- One exit point (end)
- No jumps in the middle

```rust
fn find_basic_blocks(instructions: &[Instruction]) -> Vec<BasicBlock> {
    let mut leaders = HashSet::new();
    
    // First instruction is always a leader
    leaders.insert(instructions[0].ip());
    
    // Find all jump targets and instructions after jumps
    for (idx, instr) in instructions.iter().enumerate() {
        match instr.mnemonic() {
            Mnemonic::Jmp | Mnemonic::Je | Mnemonic::Jne | /* ... */ => {
                // Jump target is a leader
                if instr.is_jmp_short_or_near() {
                    leaders.insert(instr.near_branch_target());
                }
                
                // Instruction after jump is a leader (for conditional jumps)
                if idx + 1 < instructions.len() && !is_unconditional_jmp(instr) {
                    leaders.insert(instructions[idx + 1].ip());
                }
            }
            _ => {}
        }
    }
    
    // Build blocks between leaders
    let mut blocks = Vec::new();
    let mut current_block = Vec::new();
    
    for instr in instructions {
        if leaders.contains(&instr.ip()) && !current_block.is_empty() {
            blocks.push(BasicBlock::new(current_block));
            current_block = Vec::new();
        }
        current_block.push(instr.clone());
    }
    
    if !current_block.is_empty() {
        blocks.push(BasicBlock::new(current_block));
    }
    
    blocks
}
```

### Step 2: Build Control Flow Graph

```rust
struct ControlFlowGraph {
    blocks: Vec<BasicBlock>,
    edges: HashMap<usize, Vec<Edge>>,
}

#[derive(Debug, Clone)]
enum Edge {
    Fallthrough,        // Next instruction
    ConditionalJump,    // Branch taken
    UnconditionalJump,  // Always taken
}

impl ControlFlowGraph {
    fn build(instructions: &[Instruction]) -> Self {
        let blocks = find_basic_blocks(instructions);
        let mut edges = HashMap::new();
        
        for (block_idx, block) in blocks.iter().enumerate() {
            let last_instr = block.instructions.last().unwrap();
            
            match last_instr.mnemonic() {
                Mnemonic::Jmp => {
                    // Find target block
                    let target_addr = last_instr.near_branch_target();
                    let target_idx = find_block_by_addr(&blocks, target_addr);
                    edges.entry(block_idx).or_insert(vec![])
                        .push((target_idx, Edge::UnconditionalJump));
                }
                
                Mnemonic::Je | Mnemonic::Jne | /* other conditional */ => {
                    let target_addr = last_instr.near_branch_target();
                    let target_idx = find_block_by_addr(&blocks, target_addr);
                    
                    edges.entry(block_idx).or_insert(vec![])
                        .push((target_idx, Edge::ConditionalJump));
                    
                    // Also add fallthrough edge
                    if block_idx + 1 < blocks.len() {
                        edges.entry(block_idx).or_insert(vec![])
                            .push((block_idx + 1, Edge::Fallthrough));
                    }
                }
                
                Mnemonic::Ret => {
                    // No edges - function ends
                }
                
                _ => {
                    // Fallthrough to next block
                    if block_idx + 1 < blocks.len() {
                        edges.entry(block_idx).or_insert(vec![])
                            .push((block_idx + 1, Edge::Fallthrough));
                    }
                }
            }
        }
        
        Self { blocks, edges }
    }
}
```

### Step 3: Convert to Structured Control Flow

For simple cases, use pattern matching:

```rust
impl ControlFlowGraph {
    fn to_structured_wasm(&self) -> Vec<WasmInstr> {
        let mut wasm = Vec::new();
        
        // Simple pattern: if-then-else
        if self.is_if_then_else() {
            wasm.extend(self.generate_if_then_else());
        }
        // Pattern: loop
        else if self.is_loop() {
            wasm.extend(self.generate_loop());
        }
        // Fallback: use WASM br_table for complex jumps
        else {
            wasm.extend(self.generate_br_table());
        }
        
        wasm
    }
    
    fn is_if_then_else(&self) -> bool {
        // Check if CFG matches if-then-else pattern:
        // Block 0 -> Block 1 (then) or Block 2 (else)
        // Block 1 -> Block 3 (merge)
        // Block 2 -> Block 3 (merge)
        
        if self.blocks.len() < 3 {
            return false;
        }
        
        let edges_0 = self.edges.get(&0).unwrap_or(&vec![]);
        if edges_0.len() != 2 {
            return false;
        }
        
        // Check that both branches merge at the same block
        let then_block = edges_0[0].0;
        let else_block = edges_0[1].0;
        
        let then_next = self.edges.get(&then_block).map(|e| e[0].0);
        let else_next = self.edges.get(&else_block).map(|e| e[0].0);
        
        then_next.is_some() && then_next == else_next
    }
    
    fn generate_if_then_else(&self) -> Vec<WasmInstr> {
        let mut wasm = Vec::new();
        
        // Get condition from last instruction of first block
        let cond_block = &self.blocks[0];
        let cond_instr = cond_block.instructions.last().unwrap();
        
        // Generate condition
        wasm.extend(self.translate_condition(cond_instr));
        
        // If instruction
        wasm.push(WasmInstr::If(BlockType::Empty));
        
        // Then block
        let then_idx = self.edges[&0][0].0;
        wasm.extend(self.translate_block(&self.blocks[then_idx]));
        
        // Else block
        wasm.push(WasmInstr::Else);
        let else_idx = self.edges[&0][1].0;
        wasm.extend(self.translate_block(&self.blocks[else_idx]));
        
        wasm.push(WasmInstr::End);
        
        wasm
    }
    
    fn generate_loop(&self) -> Vec<WasmInstr> {
        let mut wasm = Vec::new();
        
        // WASM loop: (loop $label ... (br $label) ...)
        wasm.push(WasmInstr::Block(BlockType::Empty));  // Outer block for break
        wasm.push(WasmInstr::Loop(BlockType::Empty));    // Loop itself
        
        // Loop body
        for block in &self.blocks {
            wasm.extend(self.translate_block(block));
        }
        
        // Back edge (if present)
        // br 1 breaks to outer block
        // br 0 continues loop
        
        wasm.push(WasmInstr::End);  // End loop
        wasm.push(WasmInstr::End);  // End block
        
        wasm
    }
    
    fn generate_br_table(&self) -> Vec<WasmInstr> {
        // For complex control flow, use a dispatch loop
        // This is the "Relooper" approach
        
        let mut wasm = Vec::new();
        
        // Local to track which block to execute
        let pc_local = 0; // Assume we allocate a local for program counter
        
        // Outer loop
        wasm.push(WasmInstr::Block(BlockType::Empty));
        wasm.push(WasmInstr::Loop(BlockType::Empty));
        
        // Load PC
        wasm.push(WasmInstr::LocalGet(pc_local));
        
        // Branch table
        let targets: Vec<u32> = (0..self.blocks.len() as u32).collect();
        wasm.push(WasmInstr::BrTable(
            targets.clone().into(),
            targets.last().copied().unwrap(),
        ));
        
        // Each block
        for (idx, block) in self.blocks.iter().enumerate() {
            wasm.push(WasmInstr::Block(BlockType::Empty));
            
            // Translate block
            wasm.extend(self.translate_block(block));
            
            // Set PC to next block based on edges
            if let Some(edges) = self.edges.get(&idx) {
                if edges.len() == 1 {
                    // Unconditional
                    wasm.push(WasmInstr::I32Const(edges[0].0 as i32));
                    wasm.push(WasmInstr::LocalSet(pc_local));
                } else {
                    // Conditional - need to evaluate condition
                    // This is where you'd translate the conditional jump
                }
            } else {
                // No edges - must be return
                wasm.push(WasmInstr::Br(2)); // Break out of loop
            }
            
            wasm.push(WasmInstr::Br(1)); // Continue loop
            wasm.push(WasmInstr::End);
        }
        
        wasm.push(WasmInstr::End); // End loop
        wasm.push(WasmInstr::End); // End block
        
        wasm
    }
    
    fn translate_condition(&self, instr: &Instruction) -> Vec<WasmInstr> {
        // Translate x86 conditional jump to WASM condition
        // This depends on the previous CMP/TEST instruction
        
        let mut wasm = Vec::new();
        
        match instr.mnemonic() {
            Mnemonic::Je => {
                // Jump if equal (zero flag set)
                // Previous CMP subtracted, so check if result is zero
                wasm.push(WasmInstr::LocalGet(FLAG_REG));
                wasm.push(WasmInstr::I64Eqz);
            }
            Mnemonic::Jne => {
                // Jump if not equal
                wasm.push(WasmInstr::LocalGet(FLAG_REG));
                wasm.push(WasmInstr::I64Eqz);
                wasm.push(WasmInstr::I32Eqz); // Negate
            }
            Mnemonic::Jg => {
                // Jump if greater (signed)
                wasm.push(WasmInstr::LocalGet(FLAG_REG));
                wasm.push(WasmInstr::I64Const(0));
                wasm.push(WasmInstr::I64GtS);
            }
            Mnemonic::Jl => {
                // Jump if less (signed)
                wasm.push(WasmInstr::LocalGet(FLAG_REG));
                wasm.push(WasmInstr::I64Const(0));
                wasm.push(WasmInstr::I64LtS);
            }
            _ => {}
        }
        
        wasm
    }
}
```

## Handling Function Calls

For sub-function calls, you have two options:

### Option 1: Inline Small Functions

```rust
fn should_inline(fn_size: usize) -> bool {
    fn_size < 100 // Inline functions smaller than 100 bytes
}

fn transpile_with_inlining(&self, fn_name: &str) -> Result<Vec<u8>, Error> {
    let mut visited = HashSet::new();
    self.transpile_recursive(fn_name, &mut visited)
}

fn transpile_recursive(&self, fn_name: &str, visited: &mut HashSet<String>) -> Result<Vec<u8>, Error> {
    if visited.contains(fn_name) {
        return Err("Recursive function detected".into());
    }
    visited.insert(fn_name.to_string());
    
    let (code, addr) = self.extract_function_code(fn_name)?;
    let instructions = self.disassemble(code, addr)?;
    
    // Find CALL instructions
    for instr in &instructions {
        if instr.mnemonic() == Mnemonic::Call {
            let target = self.resolve_call_target(&instr)?;
            
            if should_inline(target.size) {
                // Recursively transpile and inline
                self.transpile_recursive(&target.name, visited)?;
            } else {
                // Add to imports
                self.add_import(&target.name);
            }
        }
    }
    
    // Continue with normal transpilation
    Ok(vec![])
}
```

### Option 2: WASM Function References

```rust
struct WasmModuleBuilder {
    functions: Vec<WasmFunction>,
    function_map: HashMap<String, u32>, // fn_name -> function index
}

impl WasmModuleBuilder {
    fn add_function(&mut self, name: String, code: Vec<WasmInstr>) -> u32 {
        let idx = self.functions.len() as u32;
        self.functions.push(WasmFunction { name: name.clone(), code });
        self.function_map.insert(name, idx);
        idx
    }
    
    fn translate_call(&self, instr: &Instruction) -> Result<Vec<WasmInstr>, Error> {
        let target_name = self.resolve_call_target(instr)?;
        
        if let Some(&fn_idx) = self.function_map.get(&target_name) {
            // Call internal function
            Ok(vec![WasmInstr::Call(fn_idx)])
        } else {
            // Call imported function
            let import_idx = self.get_or_add_import(&target_name);
            Ok(vec![WasmInstr::Call(import_idx)])
        }
    }
}
```

## Complete Example: Simple Callback

Let's transpile this C function:

```c
int increment_if_positive(int* ptr) {
    int val = *ptr;
    if (val > 0) {
        val = val + 1;
    } else {
        val = 0;
    }
    *ptr = val;
    return val;
}
```

Assembly (simplified):
```asm
increment_if_positive:
    mov eax, [rdi]        ; Load *ptr into eax
    cmp eax, 0            ; Compare with 0
    jle .else_block       ; If <= 0, jump to else
    add eax, 1            ; val = val + 1
    jmp .merge            ; Jump to merge
.else_block:
    mov eax, 0            ; val = 0
.merge:
    mov [rdi], eax        ; Store back to *ptr
    ret                   ; Return val
```

WASM output:
```wasm
(func $increment_if_positive (param $ptr i64) (result i64)
  (local $val i64)
  
  ;; Load *ptr
  (local.set $val
    (i64.load (local.get $ptr)))
  
  ;; If val > 0
  (if (i64.gt_s (local.get $val) (i64.const 0))
    (then
      ;; val = val + 1
      (local.set $val
        (i64.add (local.get $val) (i64.const 1)))
    )
    (else
      ;; val = 0
      (local.set $val (i64.const 0))
    )
  )
  
  ;; Store back
  (i64.store (local.get $ptr) (local.get $val))
  
  ;; Return val
  (local.get $val)
)
```

## Simplified Approach for Simple Callbacks

For callbacks that are just arithmetic/loads/stores without complex control flow:

```rust
pub fn transpile_simple_callback(code: &[u8]) -> Result<Vec<u8>, Error> {
    // Skip complex CFG analysis if no jumps detected
    let has_jumps = code.iter().any(|&b| {
        b == 0x74 || b == 0x75 || b == 0xEB || // je, jne, jmp short
        b == 0x0F  // two-byte opcodes (long jumps)
    });
    
    if !has_jumps {
        // Simple linear translation
        return transpile_linear(code);
    }
    
    // Otherwise do full CFG analysis
    transpile_with_cfg(code)
}
```

This covers the main challenges! The key insight is that for simple callbacks (which you mentioned), you can often avoid the full Relooper complexity and use pattern matching on common structures.
