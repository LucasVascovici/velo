"""Unit tests for the Rust parser in check.py (run with pytest)."""
import check

SRC = '''
pub fn top() {}
pub(crate) fn hidden() {}
#[doc(hidden)]
pub fn doc_hidden() {}
fn private() {}
impl Repo {
    pub fn a(&self) { if x { y } }
    fn b(&self) {}
    #[cfg(test)]
    pub fn t(&self) {}
}
impl Trait for Repo {
    pub fn not_inherent(&self) {}
}
impl Other {
    pub fn other(&self) {}
}
impl WriteGuard<'_> {
    pub fn g(&self) { let s = "}"; let c = '{'; }
}
mod tests { pub fn inner() {} }
pub fn after() {}
'''


def test_module_fns():
    assert check.parse_entries(SRC, "m", False) == ["m::top", "m::after"]


def test_impl_methods():
    got = check.parse_entries(SRC, None, True)
    assert got == ["Repo::a", "WriteGuard::g"]


def test_strip_blanks_literals():
    assert check.strip('let s = "}";') == "let s =    ;"
    assert "{" not in check.strip("let c = '{';")
    assert "{" in check.strip('x("http://a") {')
    assert check.strip("let u = \"a//b\"; {") .endswith("; {")
    lt = "fn f<'a>(x: &'a str) {"
    assert check.strip(lt) == lt


def test_literals_do_not_skew_depth():
    src = (
        "impl Repo {\n"
        '    pub fn a(&self) {\n        let s = "{";\n    }\n'
        "    pub fn b(&self) {\n        let c = '{';\n    }\n"
        '    pub fn c(&self) {\n        let u = "http://x {";\n    }\n'
        "    pub fn d(&self) {}\n"
        "}\n"
        "pub fn after() {}\n"
    )
    assert check.parse_entries(src, None, True) == [
        "Repo::a", "Repo::b", "Repo::c", "Repo::d",
    ]
    assert check.parse_entries(src, "m", False) == ["m::after"]
