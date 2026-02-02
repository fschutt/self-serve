# Step-by-Step: Transpiling Your First Callback

This guide shows exactly how to transpile a real x86-64 C callback to WASM.

## Step 1: Write and Compile Your Callback

Create `callback.c`:
```c
int increment_counter(int* state) {
    int current = *state;
    current = current + 1;
    *state = current;
    return current;
}
```

Compile with debug symbols:
```bash
gcc -O1 -fno-inline -g -c callback.c -o callback.o
```

## Step 2: Extract the Function

Use `objdump` to see the assembly:
```bash
objdump -d callback.o
```

Output:
```asm
0000000000000000 <increment_counter>:
   0:   8b 07                   mov    eax,DWORD PTR [rdi]      ; Load *state
   2:   83 c0 01                add    eax,0x1                  ; Add 1
   5:   89 07                   mov    DWORD PTR [rdi],eax      ; Store back
   7:   c3                      ret                             ; Return
```

The function bytes are: `8b 07 83 c0 01 89 07 c3`

## Step 3: Understand the x86-64

Breaking down each instruction:

```
8b 07           mov eax, [rdi]
- OpCode: 8b (MOV r32, r/m32)
- ModRM: 07 (register indirect addressing, RDI)
- Meaning: Load 32-bit value from address in RDI into EAX

83 c0 01        add eax, 1
- OpCode: 83 (ADD r/m32, imm8)
- ModRM: c0 (register direct, EAX)
- Immediate: 01
- Meaning: Add 1 to EAX

89 07           mov [rdi], eax
- OpCode: 89 (MOV r/m32, r32)
- ModRM: 07 (register indirect, RDI)
- Meaning: Store EAX to address in RDI

c3              ret
- OpCode: c3 (RET near)
- Meaning: Return from function
```

## Step 4: Map to WASM Concepts

x86-64 -> WASM mapping:

```
Registers:
  RDI (param 0) -> local $ptr (local 0)
  EAX (return)  -> local $val (local 1)

Memory:
  [rdi]         -> (i64.load (local.get $ptr))
  [rdi] = eax   -> (i64.store (local.get $ptr) (local.get $val))

Arithmetic:
  add eax, 1    -> (i64.add (local.get $val) (i64.const 1))

Control:
  ret           -> (return (local.get $val))
```

## Step 5: Write the WASM (Text Format)

```wasm
(module
  (memory 1)
  (func $increment_counter (param $ptr i64) (result i64)
    (local $val i64)
    
    ;; mov eax, [rdi]
    (local.set $val
      (i64.load32_u (local.get $ptr)))
    
    ;; add eax, 1
    (local.set $val
      (i64.add (local.get $val) (i64.const 1)))
    
    ;; mov [rdi], eax
    (i64.store32
      (local.get $ptr)
      (local.get $val))
    
    ;; ret (return eax)
    (local.get $val)
  )
  (export "increment_counter" (func $increment_counter))
  (export "memory" (memory 0))
)
```

## Step 6: Generate Binary WASM

Using `wasm-encoder` crate:

```rust
use wasm_encoder::*;

fn generate_increment_wasm() -> Vec<u8> {
    let mut module = Module::new();
    
    // Type: (i64) -> i64
    let mut types = TypeSection::new();
    types.function(vec![ValType::I64], vec![ValType::I64]);
    module.section(&types);
    
    // Function
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    
    // Memory
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: 1,
        maximum: Some(1),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&memories);
    
    // Exports
    let mut exports = ExportSection::new();
    exports.export("increment_counter", ExportKind::Func, 0);
    exports.export("memory", ExportKind::Memory, 0);
    module.section(&exports);
    
    // Code
    let mut codes = CodeSection::new();
    let mut func = Function::new(vec![(1, ValType::I64)]); // 1 local (val)
    
    // mov eax, [rdi]
    func.instruction(&Instruction::LocalGet(0));  // Get ptr
    func.instruction(&Instruction::I32Load(MemArg {
        offset: 0,
        align: 2,  // 4-byte alignment for i32
        memory_index: 0,
    }));
    func.instruction(&Instruction::I64ExtendI32U);  // Extend to i64
    func.instruction(&Instruction::LocalSet(1));    // Set val
    
    // add eax, 1
    func.instruction(&Instruction::LocalGet(1));
    func.instruction(&Instruction::I64Const(1));
    func.instruction(&Instruction::I64Add);
    func.instruction(&Instruction::LocalSet(1));
    
    // mov [rdi], eax
    func.instruction(&Instruction::LocalGet(0));  // Get ptr
    func.instruction(&Instruction::LocalGet(1));  // Get val
    func.instruction(&Instruction::I64WrapI32);   // Truncate to i32
    func.instruction(&Instruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
    
    // ret
    func.instruction(&Instruction::LocalGet(1));
    func.instruction(&Instruction::End);
    
    codes.function(&func);
    module.section(&codes);
    
    module.finish()
}
```

## Step 7: Automated Translation

Put it all together:

```rust
pub struct CallbackTranspiler {
    binary_path: String,
}

impl CallbackTranspiler {
    pub fn transpile_callback(&self, fn_name: &str) -> Result<Vec<u8>, Error> {
        // 1. Extract function bytes
        let (bytes, addr) = self.extract_function(fn_name)?;
        
        // 2. Disassemble
        let instructions = self.disassemble(bytes, addr)?;
        
        // 3. Analyze (check for jumps, calls, etc.)
        let has_control_flow = instructions.iter().any(|i| {
            matches!(i.mnemonic(), 
                Mnemonic::Jmp | Mnemonic::Je | Mnemonic::Jne | 
                Mnemonic::Call | Mnemonic::Jg | Mnemonic::Jl
            )
        });
        
        // 4. Translate
        if has_control_flow {
            self.transpile_with_cfg(&instructions)
        } else {
            self.transpile_linear(&instructions)
        }
    }
    
    fn transpile_linear(&self, instructions: &[Instruction]) -> Result<Vec<u8>, Error> {
        let mut wasm_instrs = Vec::new();
        let mut reg_allocator = RegisterAllocator::new();
        
        for instr in instructions {
            wasm_instrs.extend(self.translate_instruction(instr, &mut reg_allocator)?);
        }
        
        Ok(self.encode_module(wasm_instrs, reg_allocator.num_locals()))
    }
    
    fn translate_instruction(&self, instr: &Instruction, alloc: &mut RegisterAllocator) 
        -> Result<Vec<WasmInstr>, Error> 
    {
        use Mnemonic::*;
        use OpKind::*;
        
        let mut wasm = Vec::new();
        
        match instr.mnemonic() {
            Mov => match (instr.op0_kind(), instr.op1_kind()) {
                (Register, Memory) => {
                    // Load from memory
                    let dst = alloc.alloc(instr.op0_register());
                    let ptr = alloc.alloc(instr.memory_base());
                    
                    wasm.push(LocalGet(ptr));
                    wasm.push(I32Load { 
                        offset: instr.memory_displacement() as u32,
                        align: 2,
                    });
                    wasm.push(I64ExtendI32U);
                    wasm.push(LocalSet(dst));
                }
                (Memory, Register) => {
                    // Store to memory
                    let src = alloc.alloc(instr.op1_register());
                    let ptr = alloc.alloc(instr.memory_base());
                    
                    wasm.push(LocalGet(ptr));
                    wasm.push(LocalGet(src));
                    wasm.push(I64WrapI32);
                    wasm.push(I32Store {
                        offset: instr.memory_displacement() as u32,
                        align: 2,
                    });
                }
                (Register, Register) => {
                    let dst = alloc.alloc(instr.op0_register());
                    let src = alloc.alloc(instr.op1_register());
                    wasm.push(LocalGet(src));
                    wasm.push(LocalSet(dst));
                }
                _ => {}
            },
            
            Add => {
                let dst = alloc.alloc(instr.op0_register());
                wasm.push(LocalGet(dst));
                wasm.push(I64Const(instr.immediate32() as i64));
                wasm.push(I64Add);
                wasm.push(LocalSet(dst));
            }
            
            Ret => {
                let rax = alloc.alloc(Register::RAX);
                wasm.push(LocalGet(rax));
                wasm.push(Return);
            }
            
            _ => return Err("Unsupported instruction".into()),
        }
        
        Ok(wasm)
    }
}
```

## Step 8: Test It

Create a test:
```rust
#[test]
fn test_increment_transpilation() {
    let transpiler = CallbackTranspiler::new("./callback.o");
    let wasm = transpiler.transpile_callback("increment_counter").unwrap();
    
    // Save for inspection
    std::fs::write("increment_counter.wasm", &wasm).unwrap();
    
    // Validate using wasmparser
    let result = wasmparser::validate(&wasm);
    assert!(result.is_ok());
}
```

Run the WASM:
```rust
use wasmer::{Store, Module, Instance, Value};

fn test_wasm_execution() {
    let wasm_bytes = std::fs::read("increment_counter.wasm").unwrap();
    
    let store = Store::default();
    let module = Module::new(&store, &wasm_bytes).unwrap();
    let instance = Instance::new(&module, &[]).unwrap();
    
    // Get memory
    let memory = instance.exports.get_memory("memory").unwrap();
    
    // Write initial value at address 0
    memory.view::<i32>()[0].set(42);
    
    // Call function
    let func = instance.exports.get_function("increment_counter").unwrap();
    let result = func.call(&[Value::I64(0)]).unwrap();
    
    // Check result
    assert_eq!(result[0], Value::I64(43));
    
    // Check memory was updated
    assert_eq!(memory.view::<i32>()[0].get(), 43);
}
```

## Common Patterns

### Pattern 1: Simple Arithmetic

```c
int add(int a, int b) { return a + b; }
```
→
```wasm
(local.get $a)
(local.get $b)
(i64.add)
```

### Pattern 2: Pointer Dereference

```c
int load(int* ptr) { return *ptr; }
```
→
```wasm
(local.get $ptr)
(i32.load)
(i64.extend_i32_u)
```

### Pattern 3: Conditional

```c
int max(int a, int b) {
    if (a > b) return a;
    return b;
}
```
→
```wasm
(local.get $a)
(local.get $b)
(i64.gt_s)
(if (result i64)
  (then (local.get $a))
  (else (local.get $b))
)
```

### Pattern 4: Loop

```c
int sum(int n) {
    int i = 0, s = 0;
    while (i < n) {
        s += i;
        i++;
    }
    return s;
}
```
→
```wasm
(block
  (loop
    (local.get $i)
    (local.get $n)
    (i64.ge_s)
    (br_if 1)  ; Exit if i >= n
    
    (local.get $s)
    (local.get $i)
    (i64.add)
    (local.set $s)
    
    (local.get $i)
    (i64.const 1)
    (i64.add)
    (local.set $i)
    
    (br 0)  ; Continue loop
  )
)
(local.get $s)
```

## Troubleshooting

### "Invalid WASM module"

Check with `wasm-objdump -d output.wasm` to see the actual instructions.

Common issues:
- Forgot `End` instruction
- Wrong type on stack
- Invalid memory alignment

### "Function not found"

Ensure:
- Function is `extern "C"`
- Function is marked `#[no_mangle]`
- Binary was compiled with symbols

### "Unsupported instruction"

Add support for the instruction in your translator. Common missing ones:
- IMUL, DIV (multiply/divide)
- SHL, SHR (shifts)
- AND, OR, XOR (bitwise)
- PUSH, POP (need stack simulation)

## Next Steps

1. Add support for more instructions
2. Handle function calls (recursive transpilation)
3. Implement proper control flow (if/else/loop)
4. Optimize generated WASM
5. Add error recovery

This approach works well for simple callbacks. For complex functions with lots of jumps, you'll need the full CFG approach from CONTROL_FLOW_GUIDE.md.
