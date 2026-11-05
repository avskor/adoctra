//! Diagrams in the editor through a diagram server: a parse lists the
//! requests it still needs, the host passes the responses back and parses
//! again; generated images come back as `data:` URIs.

use adoctra_wasm::{DiagramWorkspace, ResourceWorkspace};

const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"20\"/>";

/// The `key` of the request at `index` in the result's `diagrams.requests`.
fn request_key(json: &str, index: usize) -> String {
    let requests = &json[json.find(r#""diagrams":{"requests":["#).unwrap()..];
    let start = requests.split(r#"{"key":""#).nth(index + 1).unwrap();
    let mut key = String::new();
    let mut chars = start.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next().unwrap() {
                'n' => key.push('\n'),
                other => key.push(other),
            },
            c => key.push(c),
        }
    }
    key
}

#[test]
fn a_parse_lists_requests_and_renders_the_responses() {
    let mut ws = DiagramWorkspace::new("https://kroki.io/", "kroki_io");
    let src = "[mermaid,flow,svg]\n----\ngraph TD; A-->B\n----\n";
    let json = ws.parse_json(src, 0);
    // Until the server answers, the block is the failure listing.
    assert!(
        json.contains(r#""diagrams":{"requests":[{"key":"GET https://kroki.io/mermaid/svg/eNpLL0osyFAIcbFWcNTVtXMCACboBEE=","url":"https://kroki.io/mermaid/svg/eNpLL0osyFAIcbFWcNTVtXMCACboBEE="}],"images":{}}}"#),
        "{json}"
    );
    assert!(
        json.contains(r#""message":"Failed to generate image: waiting for the diagram server""#),
        "{json}"
    );
    assert!(json.contains(r#""name":"listing""#), "{json}");

    ws.set_response(&request_key(&json, 0), SVG);
    let json = ws.parse_json(src, 0);
    let start = src.find("----").unwrap();
    assert!(
        json.contains(&format!(
            r#""span":{{"source_id":0,"start":{start},"end":{}}},"name":"image","target":"flow.svg""#,
            src.len() - 1
        )),
        "{json}"
    );
    assert!(json.contains(r#""height":"20""#), "{json}");
    assert!(json.contains(r#""diagnostics":[]"#), "{json}");
    assert!(
        json.contains(
            r#""diagrams":{"requests":[],"images":{"flow.svg":"data:image/svg+xml;base64,PHN2Zy"#
        ),
        "{json}"
    );

    // Forgetting the responses asks again.
    ws.clear();
    assert!(ws
        .parse_json(src, 0)
        .contains(r#""key":"GET https://kroki.io/mermaid/svg/"#));
}

#[test]
fn a_failed_request_is_the_diagram_error() {
    let mut ws = DiagramWorkspace::new("https://kroki.io", "kroki_io");
    let src = "[graphviz]\n----\ndigraph { a -> }\n----\n";
    let json = ws.parse_json(src, 0);
    assert!(
        json.contains(r#""url":"https://kroki.io/graphviz/png/"#),
        "{json}"
    );
    ws.set_error(&request_key(&json, 0), "400 \"Bad Request\"");
    let json = ws.parse_json(src, 0);
    assert!(
        json.contains(r#""message":"Failed to generate image: 400 \"Bad Request\"""#),
        "{json}"
    );
    assert!(
        json.contains(r#""diagrams":{"requests":[],"images":{}}"#),
        "{json}"
    );
}

#[test]
fn the_document_chooses_its_server_and_long_code_is_posted() {
    let mut ws = DiagramWorkspace::new("https://kroki.io/", "kroki_io");
    // Distinct lines, so the compressed GET path stays over 1024.
    let code: String = (0..200)
        .map(|i| format!("n{i} -> m{};\n", i * 7919 % 1000))
        .collect();
    let src = format!(
        ":diagram-server-url: http://localhost:8000/\n\n[graphviz,big,svg]\n----\n{code}----\n\n\
         [plantuml,small,svg,preprocess=false]\n----\nA -> B\n----\n"
    );
    let json = ws.parse_json(&src, 3);
    let body = code.trim_end().replace('\n', "\\n");
    assert!(
        json.contains(&format!(
            r#"{{"key":"POST http://localhost:8000/graphviz/svg\n{body}","url":"http://localhost:8000/graphviz/svg","body":"{body}","content_type":"text/plain; charset=utf-8"}}"#
        )),
        "{json}"
    );
    assert!(
        json.contains(r#""key":"GET http://localhost:8000/plantuml/svg/"#),
        "{json}"
    );
    assert!(json.contains(r#""source_id":3"#), "{json}");

    ws.set_response(&request_key(&json, 0), SVG);
    ws.set_response(&request_key(&json, 1), SVG);
    let json = ws.parse_json(&src, 3);
    assert!(json.contains(r#""images":{"big.svg":"data:"#), "{json}");
    assert!(json.contains(r#""small.svg":"data:"#), "{json}");
}

#[test]
fn the_browser_runs_no_tools_and_has_no_files() {
    let mut ws = DiagramWorkspace::new("https://kroki.io/", "kroki_io");
    // Like Ruby, PlantUML preprocesses locally even with a server.
    let json = ws.parse_json("[plantuml]\n----\nA -> B\n----\n", 0);
    assert!(
        json.contains(r#""message":"Failed to generate image: Could not load PlantUML."#),
        "{json}"
    );
    assert!(json.contains(r#""requests":[]"#), "{json}");
    // A block macro's file is not there.
    let json = ws.parse_json("mermaid::flow.mmd[]\n", 0);
    assert!(
        json.contains(r#""message":"Failed to generate image: No such file or directory @ rb_sysopen - /flow.mmd""#),
        "{json}"
    );
    assert!(json.contains(r#""requests":[]"#), "{json}");
}

#[test]
fn resource_workspaces_read_diagram_files_from_the_buffers() {
    let mut ws = ResourceWorkspace::new();
    let main = ":imagesdir: img\n\nmermaid::diagrams/flow.mmd[svg]\n\ninclude::part.adoc[]\n\n\
                mermaid::../outside.mmd[svg]\n";
    let part = "[graphviz,dot,svg]\n----\ndigraph { a -> b }\n----\n";
    ws.set("docs/main.adoc", main).unwrap();
    ws.set("docs/diagrams/flow.mmd", "graph TD; A-->B\n")
        .unwrap();
    ws.set("docs/part.adoc", part).unwrap();
    let id = ws.load("docs/main.adoc", None).unwrap();
    // Without a server the workspace parses like Asciidoctor without
    // asciidoctor-diagram.
    let plain = ws.parse_json(id).unwrap();
    assert!(!plain.contains(r#""diagrams":"#), "{plain}");
    assert!(
        plain.contains(r#""value":"mermaid::diagrams/flow.mmd[svg]""#),
        "{plain}"
    );

    ws.enable_diagrams("https://kroki.io/", "kroki_io");
    let json = ws.parse_json(id).unwrap();
    // The macro's source comes from its buffer, the block from the include.
    assert!(
        json.contains(
            r#"{"key":"GET https://kroki.io/mermaid/svg/eNpLL0osyFAIcbFWcNTVtXMCACboBEE=","#
        ),
        "{json}"
    );
    assert!(
        json.contains(
            r#"{"key":"GET https://kroki.io/graphviz/svg/eNpLyUwvSizIUKhWSFTQtVNIUqgFADrsBaY=","#
        ),
        "{json}"
    );
    // Like Ruby's safe mode: the jail is the document's directory.
    assert!(
        json.contains(r#""message":"path has illegal reference to ancestor of jail; recovering automatically""#),
        "{json}"
    );
    assert!(
        json.contains(r#""message":"Failed to generate image: No such file or directory @ rb_sysopen - /docs/outside.mmd""#),
        "{json}"
    );
    for index in 0..2 {
        ws.set_diagram_response(&request_key(&json, index), SVG);
    }
    let json = ws.parse_json(id).unwrap();
    let start = main.find("mermaid::").unwrap();
    assert!(
        json.contains(&format!(
            r#""span":{{"source_id":{id},"start":{start},"end":{}}},"name":"image","target":"flow.svg","src":"img/flow.svg""#,
            start + "mermaid::diagrams/flow.mmd[svg]".len()
        )),
        "{json}"
    );
    assert!(
        json.contains(r#""name":"image","target":"dot.svg""#),
        "{json}"
    );
    assert!(
        json.contains(
            r#""diagrams":{"requests":[],"images":{"docs/img/dot.svg":"data:image/svg+xml;base64,"#
        ),
        "{json}"
    );
    assert!(json.contains(r#","docs/img/flow.svg":"data:"#), "{json}");
    // Without the server's answers the requests come back.
    ws.clear_diagram_responses();
    let json = ws.parse_json(id).unwrap();
    assert!(json.contains(r#""requests":[{"key":"#), "{json}");
    ws.set_diagram_error(&request_key(&json, 0), "500 \"Internal Server Error\"");
    assert!(ws
        .parse_json(id)
        .unwrap()
        .contains(r#""message":"Failed to generate image: 500 \"Internal Server Error\"""#));
}
