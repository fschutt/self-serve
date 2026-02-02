// Example: Extended state management
// This shows how to expand the PoC to handle more complex applications

use std::collections::HashMap;

// More complex application state
pub struct AppState {
    pub counter: i32,
    pub todos: Vec<Todo>,
    pub user: Option<User>,
}

pub struct Todo {
    pub id: u32,
    pub text: String,
    pub completed: bool,
}

pub struct User {
    pub name: String,
    pub email: String,
}

// Callback functions with more complex logic
#[no_mangle]
pub extern "C" fn add_todo(state_ptr: *mut AppState, text_ptr: *const u8, text_len: usize) -> u32 {
    unsafe {
        if state_ptr.is_null() || text_ptr.is_null() {
            return 0;
        }
        
        let state = &mut *state_ptr;
        let text = std::str::from_utf8_unchecked(
            std::slice::from_raw_parts(text_ptr, text_len)
        );
        
        let id = state.todos.len() as u32 + 1;
        state.todos.push(Todo {
            id,
            text: text.to_string(),
            completed: false,
        });
        
        id
    }
}

#[no_mangle]
pub extern "C" fn toggle_todo(state_ptr: *mut AppState, todo_id: u32) -> bool {
    unsafe {
        if state_ptr.is_null() {
            return false;
        }
        
        let state = &mut *state_ptr;
        
        for todo in &mut state.todos {
            if todo.id == todo_id {
                todo.completed = !todo.completed;
                return todo.completed;
            }
        }
        
        false
    }
}

#[no_mangle]
pub extern "C" fn delete_todo(state_ptr: *mut AppState, todo_id: u32) -> bool {
    unsafe {
        if state_ptr.is_null() {
            return false;
        }
        
        let state = &mut *state_ptr;
        let original_len = state.todos.len();
        
        state.todos.retain(|todo| todo.id != todo_id);
        
        state.todos.len() < original_len
    }
}

#[no_mangle]
pub extern "C" fn clear_completed(state_ptr: *mut AppState) -> u32 {
    unsafe {
        if state_ptr.is_null() {
            return 0;
        }
        
        let state = &mut *state_ptr;
        let original_len = state.todos.len();
        
        state.todos.retain(|todo| !todo.completed);
        
        (original_len - state.todos.len()) as u32
    }
}

// Complex rendering logic
fn render_app_extended(state: &AppState) -> Dom {
    let mut todos_nodes = Vec::new();
    
    for todo in &state.todos {
        let checkbox_attrs = if todo.completed {
            vec![
                ("type", "checkbox"),
                ("checked", "checked"),
                ("onclick", &format!("executeCallback('toggle_todo', {})", todo.id)),
            ]
        } else {
            vec![
                ("type", "checkbox"),
                ("onclick", &format!("executeCallback('toggle_todo', {})", todo.id)),
            ]
        };
        
        let todo_class = if todo.completed {
            "todo-item completed"
        } else {
            "todo-item"
        };
        
        todos_nodes.push(DomNode::element("li", vec![("class", todo_class)], vec![
            DomNode::element("input", checkbox_attrs, vec![]),
            DomNode::element("span", vec![], vec![
                DomNode::text(&todo.text),
            ]),
            DomNode::element("button", vec![
                ("onclick", &format!("executeCallback('delete_todo', {})", todo.id)),
                ("class", "delete-btn"),
            ], vec![
                DomNode::text("×"),
            ]),
        ]));
    }
    
    let completed_count = state.todos.iter().filter(|t| t.completed).count();
    let active_count = state.todos.len() - completed_count;
    
    Dom {
        nodes: vec![
            DomNode::element("div", vec![("class", "app-container")], vec![
                DomNode::element("h1", vec![], vec![
                    DomNode::text("x64 to WASM Todo App"),
                ]),
                
                DomNode::element("div", vec![("class", "input-section")], vec![
                    DomNode::element("input", vec![
                        ("type", "text"),
                        ("id", "new-todo"),
                        ("placeholder", "What needs to be done?"),
                    ], vec![]),
                    DomNode::element("button", vec![
                        ("onclick", "addTodo()"),
                    ], vec![
                        DomNode::text("Add"),
                    ]),
                ]),
                
                DomNode::element("ul", vec![("class", "todo-list")], todos_nodes),
                
                DomNode::element("div", vec![("class", "footer")], vec![
                    DomNode::element("span", vec![], vec![
                        DomNode::text(&format!("{} active, {} completed", active_count, completed_count)),
                    ]),
                    DomNode::element("button", vec![
                        ("onclick", "executeCallback('clear_completed')"),
                        ("class", "clear-btn"),
                    ], vec![
                        DomNode::text("Clear completed"),
                    ]),
                ]),
            ]),
        ],
    }
}

// Notes on transpilation challenges:
// 
// 1. String handling: WASM needs linear memory for strings
//    - Allocate string in WASM memory
//    - Pass pointer and length
//    
// 2. Dynamic arrays (Vec): Need heap allocation
//    - Implement allocator in WASM
//    - Or use externref for opaque handles
//    
// 3. Complex data structures: Serialize/deserialize
//    - JSON or binary format
//    - Or share memory region
//    
// 4. Callbacks with parameters: Encode in URL or POST body
//    - /execute/add_todo?text=hello
//    - Or use JSON payload
