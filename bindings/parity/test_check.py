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
