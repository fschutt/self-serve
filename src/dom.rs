pub struct Dom {
    pub nodes: Vec<DomNode>,
}

pub enum DomNode {
    Element {
        tag: String,
        attrs: Vec<(String, String)>,
        children: Vec<DomNode>,
    },
    Text(String),
}

impl DomNode {
    pub fn element(tag: &str, attrs: Vec<(&str, &str)>, children: Vec<DomNode>) -> Self {
        DomNode::Element {
            tag: tag.to_string(),
            attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            children,
        }
    }
    
    pub fn text(content: &str) -> Self {
        DomNode::Text(content.to_string())
    }
    
    fn to_html(&self) -> String {
        match self {
            DomNode::Element { tag, attrs, children } => {
                let attrs_str = attrs
                    .iter()
                    .map(|(k, v)| format!(r#"{}="{}""#, k, v))
                    .collect::<Vec<_>>()
                    .join(" ");
                
                let attrs_part = if attrs_str.is_empty() {
                    String::new()
                } else {
                    format!(" {}", attrs_str)
                };
                
                let children_html = children
                    .iter()
                    .map(|child| child.to_html())
                    .collect::<String>();
                
                format!("<{}{}>{}</{}>", tag, attrs_part, children_html, tag)
            }
            DomNode::Text(content) => content.clone(),
        }
    }
}

impl Dom {
    pub fn to_html(&self) -> String {
        self.nodes
            .iter()
            .map(|node| node.to_html())
            .collect::<String>()
    }
}
