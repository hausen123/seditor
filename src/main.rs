use std::io::{self, Write};

#[derive(Debug)]
struct Node {
    text: String,
    children: Vec<Node>,
}

fn print_tree(node: &Node, prefix: &str, last: bool) {
    println!(
        "{}{}{}",
        prefix,
        if last { "└── " } else { "├── " },
        node.text
    );

    let next_prefix = format!(
        "{}{}",
        prefix,
        if last { "    " } else { "│   " }
    );

    for (i, child) in node.children.iter().enumerate() {
        print_tree(child, &next_prefix, i == node.children.len() - 1);
    }
}

fn main() {
    let mut root = Node {
        text: "ROOT".into(),
        children: vec![],
    };

    let mut stack: Vec<*mut Node> = vec![&mut root];

    println!("Tabで階層を作ります。空行で終了。");

    loop {
        print!("> ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();

        let input = input.trim_end();

        if input.is_empty() {
            break;
        }

        let depth = input.chars().take_while(|c| *c == '\t').count();
        let text = input.trim_start_matches('\t');

        while stack.len() > depth + 1 {
            stack.pop();
        }

        let parent = unsafe { &mut *stack[depth] };

        parent.children.push(Node {
            text: text.to_string(),
            children: vec![],
        });

        let child = parent.children.last_mut().unwrap();
        stack.push(child);
    }

    println!("\nTree:");

    for (i, child) in root.children.iter().enumerate() {
        print_tree(child, "", i == root.children.len() - 1);
    }
}
