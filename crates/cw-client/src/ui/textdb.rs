//! The text database: `cube::Speech` (vtable 0x00703b70, ctor `Cube.exe 0x004e1970`), the
//! dictionary every keyed game text comes from (`description:<skill>`, `details:<skill>`,
//! `skill:level`, `specialization:<class>:<spec>`, quest and NPC speech, creature and item
//! names, landscape names).
//!
//! # Where it comes from
//!
//! The client's `cube::World` (embedded at `GameController+0x2e4`, ctor 0x0058eb00) owns a
//! `Speech` at `World+0x30` (`GC+0x314`). Its ctor opens `data4.db` (0x00703b74), reads the
//! blob `dict_en.xml` (0x00703b80; `dict_de.xml` is never referenced, so the Options
//! "language" value, index 10, does not select the dictionary) and parses it with pugixml
//! (`load_buffer` 0x004d7b00, default options: escapes and end-of-line normalisation on,
//! comments, declarations and whitespace-only PCDATA dropped). It walks the children of the
//! document element (`<root>`); an element whose `key` attribute is empty is skipped, the
//! others are dispatched on their name:
//!
//! | Element | Stored as |
//! |---|---|
//! | `name` | forms `child name -> child_value` plus the whitespace-split `tags` attribute |
//! | `sentence` | `child_value` (not present in the shipped dictionary) |
//! | `landscape` | the `child_value`s of `normal`, `in`, `to`, `the` |
//! | `quest` | `child name -> QuestText(child_value)` (not present in the shipped dictionary) |
//! | `speech` | `QuestText(child_value)` in the map at `Speech+0x44` |
//!
//! # The speech markup (`cube::QuestText`, 0x10 bytes, ctor 0x004da380; parser 0x004da850)
//!
//! A speech text is tokenised on whitespace, the punctuation `. : - , ; ! ? / ( )`
//! (0x004da800) and the brackets `{ } [ ] |`, into a tree of `cube::QuestTextNode`s (0x44
//! bytes, ctor 0x004da3b0):
//!
//! * `[ ... ]` opens a group node (kind 3); `$style` tokens inside it colour its text
//!   children (`$creature`, `$name`, `$item`, `$object`, `$zone`, `$stress`, `$number`);
//! * `{ a | b | c }` opens a choice node (kind 2) whose alternatives are group nodes
//!   (kind 0); `#tag` tokens make the node that is open conditional on a tag;
//! * any other token is a text node; `@key` text nodes are replaced by the variable `key`
//!   when rendered and `%` starts a new line; a punctuation delimiter becomes its own node
//!   (kind 1) after the token before it.
//!
//! Rendering (0x004e4a20 / 0x004e4350) appends coloured words ([`Word`]) to a vector of
//! lines ([`Lines`]); the tooltip functions of [`super::tooltip`] lay those out.
//!
//! Tier B. Strings are Rust `String`s where the original holds `std::wstring` (UTF-16); the
//! ordered sets compare by code point instead of UTF-16 unit, which only differs for
//! characters above U+FFFF (none in the dictionaries).

use std::collections::{BTreeMap, BTreeSet};

/// One laid-out word: the text and its RGBA colour (`std::pair<wstring, float4>` in the
/// line lists 0x004e5740 builds).
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    /// The word (no whitespace; may be empty, see [`push_words`]).
    pub text: String,
    /// RGBA.
    pub color: [f32; 4],
}

/// The line vector the renderer fills (`std::vector<std::list<Word>>`, 8-byte elements).
pub type Lines = Vec<Vec<Word>>;

/// The default text colour: white.
pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// The `$style` colours of 0x004e4350 (0x004e4670..0x004e4875), in the order they are
/// tested. `0.3` is `0x3e99999a`, `0.6` is `0x3f19999a`.
pub const STYLES: [(&str, [f32; 4]); 7] = [
    ("$creature", [1.0, 0.3, 0.3, 1.0]),
    ("$name", [0.6, 0.3, 1.0, 1.0]),
    ("$item", [0.3, 0.3, 1.0, 1.0]),
    ("$object", [1.0, 0.5, 0.3, 1.0]),
    ("$zone", [0.3, 1.0, 0.3, 1.0]),
    ("$stress", [0.6, 0.3, 1.0, 1.0]),
    ("$number", [0.3, 1.0, 0.3, 1.0]),
];

/// `QuestTextNode+0x40`: the node kind the parser records (the renderer does not read it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NodeKind {
    /// 0: the root, a text node or a choice alternative.
    #[default]
    Plain,
    /// 1: a punctuation node.
    Punctuation,
    /// 2: a `{ }` choice.
    Choice,
    /// 3: a `[ ]` group.
    Group,
}

/// `cube::QuestTextNode` (0x44 bytes, vtable `cube::QuestTextNode::vftable`, ctor
/// 0x004da3b0).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextNode {
    /// +4: `#tag` conditions (`std::set<wstring>`); the node renders only when one of them
    /// is in the tag set.
    pub conditions: BTreeSet<String>,
    /// +0xc: the conditions of the node and its whole subtree (filled by 0x004da770; the
    /// renderer looks them up without effect).
    pub subtree_conditions: BTreeSet<String>,
    /// +0x14: `$style` tokens; they colour the node's text children.
    pub styles: BTreeSet<String>,
    /// +0x1c: the text (`std::wstring`, empty for group nodes).
    pub text: String,
    /// +0x34: the parent node (index into [`QuestText::nodes`]).
    pub parent: Option<usize>,
    /// +0x38: the children in order (`std::list<QuestTextNode*>`).
    pub children: Vec<usize>,
    /// +0x40.
    pub kind: NodeKind,
}

/// `cube::QuestText` (0x10 bytes, ctor 0x004da380): +4 the root node, +8 every `#tag` used
/// in the text. Nodes live in an arena; index 0 is the root.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestText {
    /// The node arena (`nodes[0]` is `QuestText+4`).
    pub nodes: Vec<TextNode>,
    /// +8: the union of every node's conditions (0x004da770; empty when the parse aborted).
    pub all_tags: BTreeSet<String>,
}

/// 0x004da800: the punctuation that ends a token and becomes its own node.
pub fn is_punct(c: char) -> bool {
    matches!(c, '.' | ':' | '-' | ',' | ';' | '!' | '?' | '/' | '(' | ')')
}

impl QuestText {
    fn add_child(&mut self, parent: usize, kind: NodeKind, text: String) -> usize {
        let id = self.nodes.len();
        self.nodes.push(TextNode { kind, text, parent: Some(parent), ..Default::default() });
        self.nodes[parent].children.push(id);
        id
    }

    /// `QuestText::parse` 0x004da850. The node stack is a `std::list<QuestTextNode*>`
    /// holding the root; the loop runs over `i = 0..=len` so the terminator ends the last
    /// token. A `|` with fewer than 2, a `}` with fewer than 3 or a `]` with fewer than 2
    /// entries on the stack aborts the parse (0x004db0f6): the nodes built so far stay and
    /// the post-pass 0x004da770 is skipped.
    pub fn parse(text: &str) -> QuestText {
        let mut q = QuestText { nodes: vec![TextNode::default()], all_tags: BTreeSet::new() };
        let s: Vec<char> = text.chars().collect();
        let len = s.len() as i32;
        let mut stack: Vec<usize> = vec![0];
        // `local_50`: the index of the previous delimiter.
        let mut last: i32 = -1;
        let mut i: i32 = 0;
        while i <= len {
            let c = if i < len { s[i as usize] } else { '\0' };
            // 0x004da942..: space, '\n', '\t', '\0', punctuation or a bracket ends a token.
            let delim = c == ' '
                || c == '\n'
                || c == '\t'
                || c == '\0'
                || is_punct(c)
                || matches!(c, '{' | '}' | '[' | ']' | '|');
            if !delim {
                i += 1;
                continue;
            }
            // 0x004da9f8: '{' opens a choice (kind 2) and its first alternative (kind 0).
            if c == '{' {
                let top = *stack.last().unwrap();
                let choice = q.add_child(top, NodeKind::Choice, String::new());
                stack.push(choice);
                let alt = q.add_child(choice, NodeKind::Plain, String::new());
                stack.push(alt);
            }
            // '[' opens a group (kind 3).
            if c == '[' {
                let top = *stack.last().unwrap();
                let g = q.add_child(top, NodeKind::Group, String::new());
                stack.push(g);
            }
            // 0x004dac89: the token (also an empty one before punctuation) goes to the node
            // on top of the stack, which is already the bracket just opened.
            if last + 1 < i || is_punct(c) {
                let tok: String = s[(last + 1) as usize..i as usize].iter().collect();
                let top = *stack.last().unwrap();
                if tok.starts_with('#') {
                    q.nodes[top].conditions.insert(tok);
                } else if tok.starts_with('$') {
                    q.nodes[top].styles.insert(tok);
                } else {
                    q.add_child(top, NodeKind::Plain, tok);
                    if is_punct(c) {
                        q.add_child(top, NodeKind::Punctuation, c.to_string());
                    }
                }
            }
            last = i;
            // 0x004daee3: '|' closes the alternative and opens the next one.
            if c == '|' {
                if stack.len() < 2 {
                    return q;
                }
                stack.pop();
                let top = *stack.last().unwrap();
                let alt = q.add_child(top, NodeKind::Plain, String::new());
                stack.push(alt);
            }
            // 0x004dafd0: '}' closes the alternative and the choice.
            if c == '}' {
                if stack.len() < 3 {
                    return q;
                }
                stack.pop();
                stack.pop();
            }
            // 0x004db03e: ']' closes the group.
            if c == ']' {
                if stack.len() < 2 {
                    return q;
                }
                stack.pop();
            }
            i += 1;
        }
        // 0x004db08d: the tag post-pass.
        q.collect_tags(0);
        q
    }

    /// 0x004da770: `all_tags ∪= node.conditions`, `node.subtree = node.conditions`, then for
    /// each child (recursively first) `node.subtree ∪= child.subtree`.
    fn collect_tags(&mut self, node: usize) {
        let conds = self.nodes[node].conditions.clone();
        self.all_tags.extend(conds.iter().cloned());
        self.nodes[node].subtree_conditions = conds;
        let children = self.nodes[node].children.clone();
        for c in children {
            self.collect_tags(c);
            let sub = self.nodes[c].subtree_conditions.clone();
            self.nodes[node].subtree_conditions.extend(sub);
        }
    }

    /// `renderNode` 0x004e4350 on `node`, appending to `lines` (which must hold at least
    /// one line). Returns `false` when the node had conditions and one matched: the parent
    /// then stops rendering its remaining children (so the first matching alternative of a
    /// choice wins); unconditional and non-matching nodes return `true`.
    pub fn render_node(
        &self,
        node: usize,
        tags: &BTreeSet<String>,
        vars: &BTreeMap<String, String>,
        lines: &mut Lines,
    ) -> bool {
        let n = &self.nodes[node];
        // 0x004e43bc: the +0xc lookups have no effect.
        // 0x004e444d: conditions.
        let mut matched = false;
        if !n.conditions.is_empty() {
            let mut any = false;
            for c in &n.conditions {
                if tags.contains(c) {
                    any = true;
                }
            }
            if !any {
                return true;
            }
            matched = true;
        }
        // 0x004e45a5: the text.
        if !n.text.is_empty() {
            if n.text == "%" {
                // 0x004e45cf: a new empty line.
                lines.push(Vec::new());
            } else {
                // 0x004e4626: the colour from the parent's styles, the last match winning.
                let mut color = WHITE;
                if let Some(p) = n.parent {
                    for s in &self.nodes[p].styles {
                        for (name, c) in STYLES {
                            if s == name {
                                color = c;
                            }
                        }
                    }
                }
                let line = lines.last_mut().expect("render_node needs a line");
                if n.text.starts_with('@') {
                    // 0x004e48e2: a variable; an unknown one renders nothing.
                    if let Some(v) = vars.get(&n.text) {
                        push_words(line, v, color);
                    }
                } else {
                    push_words(line, &n.text, color);
                }
            }
        }
        // 0x004e49b5: the children until one returns false.
        for &c in &n.children {
            if !self.render_node(c, tags, vars, lines) {
                break;
            }
        }
        !matched
    }
}

/// 0x004e5740: split `text` with a `std::wstringstream` and `>>` and append every word with
/// `color` to `line`. The loop tests the stream state before each read and appends after
/// it, so trailing whitespace (or an empty text) appends one empty word. Whitespace is the
/// C locale's (`\t \n \v \f \r` and space).
pub fn push_words(line: &mut Vec<Word>, text: &str, color: [f32; 4]) {
    let s: Vec<char> = text.chars().collect();
    let ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r');
    let mut pos = 0usize;
    // The stream state: false once eof or fail is set.
    let mut good = true;
    while good {
        while pos < s.len() && ws(s[pos]) {
            pos += 1;
        }
        let start = pos;
        while pos < s.len() && !ws(s[pos]) {
            pos += 1;
        }
        if pos >= s.len() {
            good = false;
        }
        line.push(Word { text: s[start..pos].iter().collect(), color });
    }
}

/// A `name` entry: forms by child element name (`singular`, `plural`, `item`, `m`, ...) and
/// the whitespace-split `tags` attribute (`m`, `f`, `an`, ...).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NameEntry {
    /// `child name -> child_value`.
    pub forms: BTreeMap<String, String>,
    /// The `tags` attribute split on whitespace, empty words dropped.
    pub tags: Vec<String>,
}

/// A `landscape` entry: the four phrasings (`@` is replaced by the name elsewhere).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LandscapeEntry {
    /// `<normal>`.
    pub normal: String,
    /// `<in>`.
    pub in_: String,
    /// `<to>`.
    pub to: String,
    /// `<the>`.
    pub the: String,
}

/// `cube::Speech`: the parsed dictionary.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextDb {
    /// `Speech+0x44`: `speech` texts by key.
    pub speech: BTreeMap<String, QuestText>,
    /// `name` entries by key.
    pub names: BTreeMap<String, NameEntry>,
    /// `landscape` entries by key.
    pub landscapes: BTreeMap<String, LandscapeEntry>,
    /// `sentence` texts by key.
    pub sentences: BTreeMap<String, String>,
    /// `quest` entries by key: `child name -> QuestText`.
    pub quests: BTreeMap<String, BTreeMap<String, QuestText>>,
}

/// The asset key the ctor reads (0x00703b80).
pub const DICT_KEY: &str = "dict_en.xml";

impl TextDb {
    /// The ctor 0x004e1970 over `data4.db`.
    pub fn load(db: &cw_formats::db::AssetDb) -> Result<TextDb, String> {
        let bytes = db.get(DICT_KEY).map_err(|e| e.to_string())?;
        TextDb::from_xml(&bytes)
    }

    /// The ctor's parse of the dictionary bytes (0x004e1bdb..0x004e2735).
    pub fn from_xml(bytes: &[u8]) -> Result<TextDb, String> {
        let doc = xml::parse(bytes)?;
        let mut db = TextDb::default();
        // `doc.first_child()`: the document element.
        let Some(root) = doc.into_iter().next() else {
            return Ok(db);
        };
        for child in root.elements() {
            let key = child.attr("key").unwrap_or("");
            if key.is_empty() {
                continue;
            }
            // 0x004e1cf9: "name".
            if child.name == "name" {
                let mut e = NameEntry::default();
                for f in child.elements() {
                    e.forms.insert(f.name.clone(), f.child_value().to_string());
                }
                if let Some(t) = child.attr("tags") {
                    e.tags = t.split([' ', '\t', '\n', '\x0b', '\x0c', '\r']).filter(|w| !w.is_empty()).map(String::from).collect();
                }
                db.names.insert(key.to_string(), e);
            }
            // 0x004e21da: "sentence".
            if child.name == "sentence" {
                db.sentences.insert(key.to_string(), child.child_value().to_string());
            }
            // 0x004e223a: "landscape": normal, in, to, the (missing children read "").
            if child.name == "landscape" {
                let v = |n: &str| child.child(n).map(|c| c.child_value().to_string()).unwrap_or_default();
                db.landscapes.insert(
                    key.to_string(),
                    LandscapeEntry { normal: v("normal"), in_: v("in"), to: v("to"), the: v("the") },
                );
            }
            // 0x004e23da: "quest".
            if child.name == "quest" {
                let mut m = BTreeMap::new();
                for f in child.elements() {
                    m.insert(f.name.clone(), QuestText::parse(f.child_value()));
                }
                db.quests.insert(key.to_string(), m);
            }
            // 0x004e25e0: "speech" (`Speech+0x44[key] = new QuestText`, a later duplicate
            // replaces the earlier one).
            if child.name == "speech" {
                db.speech.insert(key.to_string(), QuestText::parse(child.child_value()));
            }
        }
        Ok(db)
    }

    /// `Speech::render` 0x004e4a20: look `key` up (0x00661830 is the map's `operator[]`; a
    /// missing key maps to a null text and renders nothing, without adding a line); if the
    /// line vector is empty push an empty line, then render the root node with `tags` (a
    /// `std::set<wstring>`, empty for tooltips) and `vars` (`@key -> value`).
    pub fn render(&self, key: &str, tags: &BTreeSet<String>, vars: &BTreeMap<String, String>, lines: &mut Lines) {
        let Some(q) = self.speech.get(key) else {
            return;
        };
        if lines.is_empty() {
            lines.push(Vec::new());
        }
        q.render_node(0, tags, vars, lines);
    }
}

/// A minimal XML reader with pugixml's default-option behaviour for what the dictionaries
/// use: elements, attributes (single or double quotes), text, CDATA, the five predefined
/// entities and numeric character references; the declaration, processing instructions,
/// comments and `<!DOCTYPE>` are dropped, and so is whitespace-only text. End of lines are
/// normalised (`\r\n` and `\r` to `\n`, `parse_eol`); in attribute values `\t \n \r` become
/// spaces (`parse_wconv_attribute`).
pub mod xml {
    /// An element.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Element {
        /// Tag name.
        pub name: String,
        /// Attributes in document order.
        pub attrs: Vec<(String, String)>,
        /// Children in document order.
        pub children: Vec<Node>,
    }

    /// A child node.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Node {
        /// An element.
        Element(Element),
        /// PCDATA or CDATA.
        Text(String),
    }

    impl Element {
        /// The first attribute named `n`.
        pub fn attr(&self, n: &str) -> Option<&str> {
            self.attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
        }

        /// The element children.
        pub fn elements(&self) -> impl Iterator<Item = &Element> {
            self.children.iter().filter_map(|c| match c {
                Node::Element(e) => Some(e),
                Node::Text(_) => None,
            })
        }

        /// The first element child named `n` (`xml_node::child`).
        pub fn child(&self, n: &str) -> Option<&Element> {
            self.elements().find(|e| e.name == n)
        }

        /// `xml_node::child_value`: the first text child, or "".
        pub fn child_value(&self) -> &str {
            for c in &self.children {
                if let Node::Text(t) = c {
                    return t;
                }
            }
            ""
        }
    }

    fn unescape(s: &str, attr: bool) -> String {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        while let Some(p) = rest.find('&') {
            out.push_str(&rest[..p]);
            let tail = &rest[p..];
            let end = tail.find(';');
            let ent = end.map(|e| &tail[1..e]);
            let rep = match ent {
                Some("lt") => Some('<'),
                Some("gt") => Some('>'),
                Some("amp") => Some('&'),
                Some("apos") => Some('\''),
                Some("quot") => Some('"'),
                Some(e) if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
                Some(e) if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            match (rep, end) {
                (Some(c), Some(e)) => {
                    out.push(c);
                    rest = &tail[e + 1..];
                }
                _ => {
                    out.push('&');
                    rest = &tail[1..];
                }
            }
        }
        out.push_str(rest);
        if attr {
            out = out.replace(['\t', '\n'], " ");
        }
        out
    }

    /// Parses a document; returns its top-level elements.
    pub fn parse(bytes: &[u8]) -> Result<Vec<Element>, String> {
        let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut stack: Vec<Element> = vec![Element::default()];
        let mut rest = text;
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix("<!--") {
                let e = r.find("-->").ok_or("unterminated comment")?;
                rest = &r[e + 3..];
            } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
                let e = r.find("]]>").ok_or("unterminated CDATA")?;
                let t = r[..e].replace("\r\n", "\n").replace('\r', "\n");
                stack.last_mut().unwrap().children.push(Node::Text(t));
                rest = &r[e + 3..];
            } else if rest.starts_with("<?") {
                let e = rest.find("?>").ok_or("unterminated processing instruction")?;
                rest = &rest[e + 2..];
            } else if rest.starts_with("<!") {
                let e = rest.find('>').ok_or("unterminated declaration")?;
                rest = &rest[e + 1..];
            } else if let Some(r) = rest.strip_prefix("</") {
                let e = r.find('>').ok_or("unterminated end tag")?;
                let name = r[..e].trim();
                if stack.len() < 2 {
                    return Err(format!("unexpected </{name}>"));
                }
                let el = stack.pop().unwrap();
                if el.name != name {
                    return Err(format!("mismatched </{name}> for <{}>", el.name));
                }
                stack.last_mut().unwrap().children.push(Node::Element(el));
                rest = &r[e + 1..];
            } else if let Some(r) = rest.strip_prefix('<') {
                let name_end = r.find(|c: char| c.is_whitespace() || c == '>' || c == '/').ok_or("bad tag")?;
                let mut el = Element { name: r[..name_end].to_string(), ..Default::default() };
                let mut r = &r[name_end..];
                loop {
                    r = r.trim_start();
                    if let Some(x) = r.strip_prefix("/>") {
                        stack.last_mut().unwrap().children.push(Node::Element(el));
                        rest = x;
                        break;
                    }
                    if let Some(x) = r.strip_prefix('>') {
                        stack.push(el);
                        rest = x;
                        break;
                    }
                    let eq = r.find('=').ok_or("attribute without value")?;
                    let an = r[..eq].trim().to_string();
                    let v = r[eq + 1..].trim_start();
                    let q = v.chars().next().ok_or("unterminated attribute")?;
                    if q != '"' && q != '\'' {
                        return Err("unquoted attribute".into());
                    }
                    let ve = v[1..].find(q).ok_or("unterminated attribute")?;
                    el.attrs.push((an, unescape(&v[1..1 + ve], true)));
                    r = &v[ve + 2..];
                }
            } else {
                let e = rest.find('<').unwrap_or(rest.len());
                let raw = &rest[..e];
                // parse_ws_pcdata is off: whitespace-only text is not stored.
                if !raw.chars().all(char::is_whitespace) {
                    stack.last_mut().unwrap().children.push(Node::Text(unescape(raw, false)));
                }
                rest = &rest[e..];
            }
        }
        if stack.len() != 1 {
            return Err("unclosed element".into());
        }
        let top = stack.pop().unwrap();
        Ok(top.children.into_iter().filter_map(|c| match c {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        }).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(l: &[Word]) -> Vec<&str> {
        l.iter().map(|w| w.text.as_str()).collect()
    }

    const SAMPLE: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<root>\r\n  <!--c-->\r\n\
        <name key=\"OldMan\" tags=\"m an\">\r\n <singular>old Man</singular>\r\n <plural>old Men</plural>\r\n </name>\r\n\
        <landscape key=\"Hills\"><normal>@ Hills</normal><in>in @ Hills</in></landscape>\r\n\
        <speech key=\"skill:level\">\r\n    % % [LVL @level $number] %\r\n  </speech>\r\n\
        <speech key=\"details:X\">\r\n    Lasts [@dur $number] seconds &amp; more.\r\n  </speech>\r\n\
        <speech key=\"\">ignored</speech>\r\n</root>\r\n";

    #[test]
    fn dictionary_parse() {
        let db = TextDb::from_xml(SAMPLE.as_bytes()).unwrap();
        assert_eq!(db.names["OldMan"].forms["plural"], "old Men");
        assert_eq!(db.names["OldMan"].tags, vec!["m", "an"]);
        assert_eq!(db.landscapes["Hills"].in_, "in @ Hills");
        assert_eq!(db.landscapes["Hills"].to, "");
        assert_eq!(db.speech.len(), 2);
    }

    #[test]
    fn render_lines_and_colours() {
        let db = TextDb::from_xml(SAMPLE.as_bytes()).unwrap();
        let mut vars = BTreeMap::new();
        vars.insert("@level".to_string(), "5".to_string());
        vars.insert("@dur".to_string(), "10".to_string());
        let tags = BTreeSet::new();
        let mut lines = Lines::new();
        db.render("missing", &tags, &vars, &mut lines);
        assert!(lines.is_empty());
        db.render("skill:level", &tags, &vars, &mut lines);
        // Initial line, then '%' '%' -> two new lines, words, '%' -> another line.
        assert_eq!(lines.len(), 4);
        assert_eq!(words(&lines[2]), vec!["LVL", "5"]);
        assert_eq!(lines[2][1].color, [0.3, 1.0, 0.3, 1.0]);
        db.render("details:X", &tags, &vars, &mut lines);
        assert_eq!(words(&lines[3]), vec!["Lasts", "10", "seconds", "&", "more", "."]);
        assert_eq!(lines[3][0].color, WHITE);
        assert_eq!(lines[3][1].color, [0.3, 1.0, 0.3, 1.0]);
    }

    #[test]
    fn choices_and_conditions() {
        let q = QuestText::parse("Defeat the {#m him|#f her|#n it}!");
        assert_eq!(q.all_tags.len(), 3);
        let vars = BTreeMap::new();
        let mut tags = BTreeSet::new();
        tags.insert("#f".to_string());
        let mut lines = vec![Vec::new()];
        q.render_node(0, &tags, &vars, &mut lines);
        assert_eq!(words(&lines[0]), vec!["Defeat", "the", "her", "!"]);
        // No tag: every alternative is skipped.
        let mut lines = vec![Vec::new()];
        q.render_node(0, &BTreeSet::new(), &vars, &mut lines);
        assert_eq!(words(&lines[0]), vec!["Defeat", "the", "!"]);
        // An alternative without condition renders and does not stop the others.
        let q = QuestText::parse("{a|#x b|c}");
        let mut lines = vec![Vec::new()];
        q.render_node(0, &tags, &vars, &mut lines);
        assert_eq!(words(&lines[0]), vec!["a", "c"]);
    }

    #[test]
    fn tokenizer_quirks() {
        // A token right before '[' goes inside the group; punctuation splits words; an
        // unknown variable renders nothing; "(" after nothing makes an empty text node.
        let q = QuestText::parse("ab[cd $item] x-y (@num/@amount) @nope");
        let g = q.nodes[0].children[0];
        assert_eq!(q.nodes[g].kind, NodeKind::Group);
        assert_eq!(q.nodes[q.nodes[g].children[0]].text, "ab");
        let mut vars = BTreeMap::new();
        vars.insert("@num".to_string(), "1".to_string());
        vars.insert("@amount".to_string(), "3".to_string());
        let mut lines = vec![Vec::new()];
        q.render_node(0, &BTreeSet::new(), &vars, &mut lines);
        assert_eq!(words(&lines[0]), vec!["ab", "cd", "x", "-", "y", "(", "1", "/", "3", ")"]);
        assert_eq!(lines[0][0].color, [0.3, 0.3, 1.0, 1.0]);
        // Unbalanced ']' aborts: the tag pass is skipped.
        let q = QuestText::parse("#t a ] b");
        assert!(q.all_tags.is_empty());
    }

    #[test]
    fn stream_split() {
        let mut l = Vec::new();
        push_words(&mut l, "16.67%", WHITE);
        push_words(&mut l, "a b ", WHITE);
        push_words(&mut l, "", WHITE);
        assert_eq!(words(&l), vec!["16.67%", "a", "b", "", ""]);
    }

    /// Loads the shipped dictionary when `CW_GAME_DIR` is set.
    #[test]
    fn game_dictionary() {
        let Ok(dir) = std::env::var("CW_GAME_DIR") else {
            return;
        };
        let adb = cw_formats::db::AssetDb::open(std::path::Path::new(&dir).join("data4.db")).unwrap();
        let db = TextDb::load(&adb).unwrap();
        assert!(db.speech.contains_key("skill:level"));
        assert!(db.speech.contains_key("skill:nextlevel"));
        assert!(db.speech.contains_key("description:AbilitySmash"));
        assert!(db.speech.contains_key("details:SkillPetTaming"));
        assert_eq!(db.names["Wizard"].forms["singular"], "Wizard");
        let mut vars = BTreeMap::new();
        vars.insert("@health".to_string(), "16.67%".to_string());
        let mut lines = Lines::new();
        db.render("description:SkillPetTaming", &BTreeSet::new(), &vars, &mut lines);
        db.render("details:SkillPetTaming", &BTreeSet::new(), &vars, &mut lines);
        assert_eq!(words(&lines[0]), vec!["Pet", "Master"]);
        assert_eq!(lines[0][0].color, [0.6, 0.3, 1.0, 1.0]);
        let l1 = words(&lines[1]);
        assert_eq!(&l1[..5], &["Your", "pets", "become", "stronger", "."]);
        assert_eq!(l1[l1.len() - 2], "16.67%");
        assert_eq!(l1[l1.len() - 1], ".");
    }
}
