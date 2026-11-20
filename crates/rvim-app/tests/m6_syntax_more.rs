//! M6 syntax remainder: synID()/synstack(), contained/nextgroup/transparent,
//! :syntax include and runtime syntax/*.vim consumption.

use rvim_app::Session;

fn temp(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(name);
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn synid_and_synstack_report_the_item_under_the_cursor() {
    let path = temp("rvim_m6_synid.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex("syntax on").unwrap();
    let id = s.editor.syntax_id_at(s.editor.current_bufnr(), 1, 1);
    assert!(id > 0, "fn has a syntax id");
    assert_eq!(s.editor.syntax_group_of(id), Some("Keyword"));
    let stack = s.editor.syntax_stack(s.editor.current_bufnr(), 1, 1);
    assert_eq!(stack.last().copied(), Some(id));
    // A non-keyword character has no item.
    let blank = s.editor.syntax_id_at(s.editor.current_bufnr(), 1, 3);
    assert_eq!(blank, 0, "space between fn and main is unhighlighted");
    let _ = std::fs::remove_file(path);
}

#[test]
fn synidattr_name_and_translate() {
    let path = temp("rvim_m6_synattr.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex("syntax on").unwrap();
    let out = std::env::temp_dir().join("rvim_m6_synattr.out");
    let _ = std::fs::remove_file(&out);
    let script = format!(
        "call writefile([synIDattr(synID(1,1,0),'name'), synIDtrans(synID(1,1,0))], '{}')",
        out.display()
    );
    s.ex(&script).unwrap();
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("Keyword"));
    assert!(
        lines
            .next()
            .and_then(|n| n.parse::<i64>().ok())
            .unwrap_or(0)
            > 0
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn contained_items_only_match_inside_their_container() {
    let path = temp("rvim_m6_contained.txt");
    std::fs::write(&path, "alpha beta \"alpha\"\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex("syntax on").unwrap();
    s.ex("syntax keyword MyWord alpha contained").unwrap();
    let bufnr = s.editor.current_bufnr();
    let spans = s.editor.syntax_spans(bufnr, 0);
    assert!(
        !spans.iter().any(|(_, _, g)| g == "MyWord"),
        "contained item must not match at top level: {spans:?}"
    );
    s.ex("syntax region MyRegion start=/\"/ end=/\"/ contains=MyWord")
        .unwrap();
    let spans = s.editor.syntax_spans(bufnr, 0);
    assert!(
        spans.iter().any(
            |(start, end, g)| &"alpha beta \"alpha\""[*start..*end] == "alpha" && g == "MyWord"
        ),
        "contained item matches inside the region: {spans:?}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn transparent_regions_colour_only_their_contents() {
    let path = temp("rvim_m6_transparent.txt");
    std::fs::write(&path, "x alpha y\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex("syntax on").unwrap();
    s.ex("syntax region Wrap start=/x/ end=/y/ transparent contains=MyWord")
        .unwrap();
    s.ex("syntax keyword MyWord alpha").unwrap();
    let bufnr = s.editor.current_bufnr();
    let spans = s.editor.syntax_spans(bufnr, 0);
    assert!(
        !spans.iter().any(|(_, _, g)| g == "Wrap"),
        "transparent region is not drawn: {spans:?}"
    );
    assert!(
        spans.iter().any(|(_, _, g)| g == "MyWord"),
        "contents are highlighted: {spans:?}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn nextgroup_colours_the_following_item() {
    let path = temp("rvim_m6_nextgroup.txt");
    std::fs::write(&path, "fn name\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex("syntax on").unwrap();
    s.ex("syntax keyword Leader fn nextgroup=Name skipwhite")
        .unwrap();
    s.ex("syntax keyword Name name contained").unwrap();
    let bufnr = s.editor.current_bufnr();
    let spans = s.editor.syntax_spans(bufnr, 0);
    assert!(
        spans
            .iter()
            .any(|(s, e, g)| &"fn name"[*s..*e] == "name" && g == "Name"),
        "{spans:?}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn syntax_include_merges_another_file() {
    let dir = std::env::temp_dir().join("rvim_m6_include");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("syntax")).unwrap();
    std::fs::write(
        dir.join("syntax").join("extra.vim"),
        "syn keyword Extra alpha\nhi def link Extra Keyword\n",
    )
    .unwrap();
    let path = temp("rvim_m6_include.txt");
    std::fs::write(&path, "alpha beta\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex(&format!("set runtimepath={}", dir.display())).unwrap();
    s.ex("syntax on").unwrap();
    s.ex("syntax include syntax/extra.vim").unwrap();
    let bufnr = s.editor.current_bufnr();
    let spans = s.editor.syntax_spans(bufnr, 0);
    assert!(
        spans
            .iter()
            .any(|(s, e, g)| &"alpha beta"[*s..*e] == "alpha" && g == "Extra"),
        "{spans:?}"
    );
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn runtime_syntax_file_is_used_for_an_unknown_filetype() {
    let dir = std::env::temp_dir().join("rvim_m6_runtime_syntax");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("syntax")).unwrap();
    std::fs::write(
        dir.join("syntax").join("foo.vim"),
        "syn keyword FooKeyword alpha\nhi def link FooKeyword Keyword\n",
    )
    .unwrap();
    let path = temp("rvim_m6_runtime.txt");
    std::fs::write(&path, "alpha beta\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex(&format!("set runtimepath={}", dir.display())).unwrap();
    s.ex("set filetype=foo").unwrap();
    s.ex("syntax on").unwrap();
    let bufnr = s.editor.current_bufnr();
    let spans = s.editor.syntax_spans(bufnr, 0);
    assert!(
        spans
            .iter()
            .any(|(s, e, g)| &"alpha beta"[*s..*e] == "alpha" && g == "FooKeyword"),
        "{spans:?}"
    );
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn zs_and_ze_adjust_the_drawn_span() {
    let path = temp("rvim_m6_zsze.txt");
    std::fs::write(&path, "foobar baz qux\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex("syntax on").unwrap();
    s.ex(r"syntax match Tail /foo\zsbar/").unwrap();
    s.ex(r"syntax match Head /baz\ze qux/").unwrap();
    let bufnr = s.editor.current_bufnr();
    let spans = s.editor.syntax_spans(bufnr, 0);
    assert!(
        spans
            .iter()
            .any(|(start, end, g)| &"foobar baz qux"[*start..*end] == "bar" && g == "Tail"),
        "\\zs moves the start: {spans:?}"
    );
    assert!(
        spans
            .iter()
            .any(|(start, end, g)| &"foobar baz qux"[*start..*end] == "baz" && g == "Head"),
        "\\ze keeps the span short: {spans:?}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn syntax_regions_can_create_folds() {
    let path = temp("rvim_m6_syntax_fold.txt");
    std::fs::write(&path, "#region\nbody\n#end\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex("syntax on").unwrap();
    s.ex("syntax region Block start=/^#region/ end=/^#end/ fold")
        .unwrap();
    s.ex("set foldmethod=syntax foldlevel=0").unwrap();
    s.editor.refresh_folds_for_current_window().unwrap();
    assert_eq!(s.editor.closed_fold_at(1), Some((0, 2)));
    let _ = std::fs::remove_file(path);
}

#[test]
fn sync_spec_is_accepted_and_used() {
    let path = temp("rvim_m6_sync.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let mut s = Session::new();
    s.open_file(&path).unwrap();
    s.ex("syntax on").unwrap();
    s.ex("syntax sync fromstart").unwrap();
    assert_eq!(s.editor.syntax.sync_start(), usize::MAX);
    s.ex("syntax sync minlines=5").unwrap();
    assert_eq!(s.editor.syntax.sync_start(), 5);
    let bufnr = s.editor.current_bufnr();
    assert!(s.editor.syntax_id_at(bufnr, 1, 1) > 0);
    let _ = std::fs::remove_file(path);
}
