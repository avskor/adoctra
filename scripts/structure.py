"""block-structure-v1: a deliberately content-free projection of parser trees.

This is not the official ASG. No HTML is parsed, and no inline text, attributes,
source positions or diagnostic equivalence is claimed by this profile.
"""

PROFILE = "block-structure-v1"
LEAVES = {"paragraph", "listing", "literal", "stem", "pass", "thematic_break",
          "page_break", "toc", "image", "video", "audio", "verse"}
CONTAINERS = {"preamble", "open", "example", "sidebar", "quote", "admonition"}
LISTS = {"ulist", "olist", "colist"}


def document_projection(document):
    if not isinstance(document, dict) or document.get("name") != "document":
        raise ValueError("expected JSON document")
    blocks = document["blocks"]
    require(isinstance(blocks, list))
    has_title = bool(blocks and blocks[0]["name"] == "heading" and blocks[0]["level"] == 0
                     and blocks[0].get("style") not in {"discrete", "float"})
    result = {"context": "document", "has_title": has_title,
              "blocks": [block_projection(b) for b in blocks[int(has_title):]]}
    validate_structure(result)
    return result


def item_projection(item):
    return {"checkbox": item.get("checkbox"),
            "blocks": [block_projection(b) for b in item["blocks"]]}


def block_projection(block):
    name = block["name"]
    children = block.get("blocks", [])
    metadata = block
    if name == "section":
        if not children or children[0]["name"] != "heading" or children[0]["level"] != block["level"]:
            raise ValueError("section must start with its heading")
        metadata, children = children[0], children[1:]
    elif name == "heading":
        name = "floating_title"
    elif name == "list":
        if type(block["ordered"]) is not bool:
            raise ValueError("invalid ordered list flag")
        name = "olist" if block["ordered"] else "ulist"
    elif name == "break":
        name = {"page": "page_break", "thematic": "thematic_break"}[block["variant"]]
    result = {"context": name, "id": metadata.get("id"), "roles": metadata.get("roles", [])}
    if name in {"section", "floating_title"}:
        result["level"] = block["level"]
    if name in CONTAINERS or name == "section":
        # Simple containers use a synthetic paragraph in adoctra. Ruby stores
        # that inline content on the container itself, with no child block.
        simple = block.get("simple", False)
        require(type(simple) is bool)
        if simple:
            if (len(children) != 1 or children[0]["name"] != "paragraph"
                    or children[0].get("id") is not None or children[0].get("roles", [])):
                raise ValueError("invalid simple container")
            children = []
        if name in CONTAINERS - {"preamble"}:
            result["content_model"] = "simple" if simple else "compound"
        result["blocks"] = [block_projection(b) for b in children]
    if name == "admonition":
        result["admonition"] = block["admonition-type"]
    if name in LISTS:
        result["items"] = [item_projection(item) for item in block["items"]]
    elif name == "dlist":
        result["items"] = [
            {"terms": len(item["terms"]),
             "description": item_projection(item["description"]) if item["description"] is not None else None}
            for item in block["items"]
        ]
    elif name == "table":
        result["columns"] = len(block["columns"])
        for group in ("head", "body", "foot"):
            result[group] = [[cell_projection(cell) for cell in row] for row in block[group]]
    return result


def cell_projection(cell):
    result = {"style": cell["style"],
              "colspan": 1 if cell["colspan"] is None else cell["colspan"],
              "rowspan": 1 if cell["rowspan"] is None else cell["rowspan"]}
    result["document"] = document_projection({"name": "document", "blocks": cell["blocks"]}) if "blocks" in cell else None
    return result


def validate_structure(node):
    """Reject unknown contexts and malformed snapshots, including XFAIL data."""
    if not isinstance(node, dict):
        raise ValueError("invalid structure node")
    context = node.get("context")
    keys = {"context", "id", "roles"}
    if context == "document":
        keys = {"context", "has_title", "blocks"}
        require(type(node.get("has_title")) is bool)
    else:
        require(node.get("id") is None or isinstance(node["id"], str))
        require(isinstance(node.get("roles"), list) and all(isinstance(r, str) for r in node["roles"]))
    if context in {"section", "floating_title"}:
        keys.add("level")
        require(type(node.get("level")) is int and node["level"] >= 0)
    if context in CONTAINERS | {"section", "document"}:
        keys.add("blocks")
        require(isinstance(node.get("blocks"), list))
        for child in node["blocks"]:
            validate_structure(child)
    if context in CONTAINERS - {"preamble"}:
        keys.add("content_model")
        require(node.get("content_model") in {"simple", "compound"})
    if context == "admonition":
        keys.add("admonition")
        require(node.get("admonition") in {"note", "tip", "important", "warning", "caution"})
    if context in LISTS | {"dlist"}:
        keys.add("items")
        require(isinstance(node.get("items"), list))
        for item in node["items"]:
            if context == "dlist":
                require(isinstance(item, dict) and set(item) == {"terms", "description"})
                require(type(item["terms"]) is int and item["terms"] > 0)
                if item["description"] is not None:
                    validate_item(item["description"])
            else:
                validate_item(item)
    if context == "table":
        keys.update({"columns", "head", "body", "foot"})
        require(type(node.get("columns")) is int and node["columns"] > 0)
        for group in ("head", "body", "foot"):
            require(isinstance(node.get(group), list))
            for row in node[group]:
                require(isinstance(row, list))
                for cell in row:
                    require(isinstance(cell, dict) and set(cell) == {"style", "colspan", "rowspan", "document"})
                    require(cell["style"] in {None, "asciidoc", "none", "emphasis", "header", "literal", "monospaced", "strong"})
                    for span in ("colspan", "rowspan"):
                        require(type(cell[span]) is int and cell[span] > 0)
                    if cell["document"] is not None:
                        require(isinstance(cell["document"], dict) and cell["document"].get("context") == "document")
                        validate_structure(cell["document"])
    require(context in LEAVES | CONTAINERS | LISTS | {"document", "section", "floating_title", "dlist", "table"})
    require(set(node) == keys)


def validate_item(item):
    require(isinstance(item, dict) and set(item) == {"checkbox", "blocks"})
    require(item["checkbox"] is None or type(item["checkbox"]) is bool)
    require(isinstance(item["blocks"], list))
    for block in item["blocks"]:
        validate_structure(block)


def require(condition):
    if not condition:
        raise ValueError("invalid block-structure-v1 projection")
