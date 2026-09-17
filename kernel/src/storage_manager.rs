//! Mount/partition inventory boundary over GPT and legacy partition parsers.
use crate::irq_lock::IrqMutex as Mutex;
#[derive(Clone, Copy, PartialEq, Eq)] pub struct Mount { pub id: u64, pub start_lba: u64, pub sectors: u64, pub removable: bool }
static MOUNTS: Mutex<[Option<Mount>; 8]> = Mutex::with_rank([None; 8], 10);
pub fn mount(start_lba: u64, sectors: u64, removable: bool) -> Option<u64> { if start_lba==0||sectors==0{return None} let mut m=MOUNTS.lock(); let i=m.iter_mut().position(|x|x.is_none())?; let id=i as u64+1; m[i]=Some(Mount{id,start_lba,sectors,removable}); Some(id) }
pub fn unmount(id: u64) -> bool { let mut m=MOUNTS.lock(); if let Some(x)=m.iter_mut().find(|x|x.is_some_and(|v|v.id==id)){*x=None;true}else{false} }
#[cfg(feature = "stage12-5-test")]
pub fn structural_self_test() -> bool { let Some(id)=mount(2048,10000,true) else{return false}; unmount(id) }
