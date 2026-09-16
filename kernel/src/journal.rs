//! Bounded write-ahead intent journal for crash-safe metadata updates.
use spin::Mutex;
const MAX: usize = 32;
#[derive(Clone, Copy, PartialEq, Eq)] pub struct Token(u64);
#[derive(Clone, Copy)] struct Entry { token: Token, path_hash: u64, checksum: u64, committed: bool }
static LOG: Mutex<[Option<Entry>; MAX]> = Mutex::new([None; MAX]);
static NEXT: Mutex<u64> = Mutex::new(1);
pub fn begin(path_hash: u64, checksum: u64) -> Option<Token> { let mut l=LOG.lock(); let slot=l.iter_mut().position(|e|e.is_none())?; let mut n=NEXT.lock(); let token=Token(*n); *n=n.saturating_add(1); l[slot]=Some(Entry{token,path_hash,checksum,committed:false}); Some(token) }
pub fn commit(token: Token) -> bool { let mut l=LOG.lock(); let Some(e)=l.iter_mut().flatten().find(|e|e.token==token) else{return false}; e.committed=true; true }
pub fn recover() -> usize { let mut l=LOG.lock(); let mut recovered=0; for e in l.iter_mut() { if e.is_some_and(|x|!x.committed) { *e=None; recovered+=1; } else if e.is_some_and(|x|x.committed) { *e=None; } } recovered }
#[cfg(feature = "stage1-5-test")]
pub fn structural_self_test() -> bool { let Some(a)=begin(11,22) else{return false}; let Some(b)=begin(33,44) else{return false}; commit(b) && recover()==1 && !commit(a) }
