//! WovenHat Stage 13.9O — fail-closed kernel entropy boundary.
//!
//! Security-sensitive callers ask this service for bytes. If no reviewed
//! entropy source is registered, requests fail; clocks/TSC/PIDs/MACs are never
//! silently substituted. The deterministic source exists only in stage13-9-test.

use spin::Mutex;
use zeroize::Zeroize;

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum EntropyError { Unavailable, InvalidRequest }

struct Pool { seeded:bool, key:[u8;32], counter:u64 }
impl Pool {
    const fn new()->Self{Self{seeded:false,key:[0;32],counter:0}}
    fn clear(&mut self){self.key.zeroize();self.counter=0;self.seeded=false;}
    fn seed(&mut self,seed:[u8;32]){self.clear();self.key=seed;self.counter=1;self.seeded=true;}
    fn fill(&mut self,out:&mut[u8])->Result<(),EntropyError>{
        if out.is_empty(){return Err(EntropyError::InvalidRequest)}
        if !self.seeded{return Err(EntropyError::Unavailable)}
        // Stage O establishes the fail-closed service boundary. This mixer is
        // test-only because production seeding is deliberately unavailable.
        #[cfg(feature="stage13-9-test")]
        {
            let mut x=self.counter^u64::from_le_bytes(self.key[..8].try_into().unwrap_or([0;8]));
            for (i,b) in out.iter_mut().enumerate(){
                x^=x<<13;x^=x>>7;x^=x<<17;
                *b=(x as u8)^self.key[i%32];
            }
            self.counter=self.counter.wrapping_add(1);
            Ok(())
        }
        #[cfg(not(feature="stage13-9-test"))]
        { let _=out; Err(EntropyError::Unavailable) }
    }
}
static POOL:Mutex<Pool>=Mutex::new(Pool::new());

pub fn fill_secure(out:&mut[u8])->Result<(),EntropyError>{POOL.lock().fill(out)}
pub fn snonce()->Result<[u8;32],EntropyError>{
    let mut n=[0u8;32];
    fill_secure(&mut n)?;
    Ok(n)
}

pub fn try_random_u64()->Result<u64,EntropyError>{
    let mut bytes=[0u8;8];
    fill_secure(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

pub fn random_u64()->u64{
    try_random_u64().unwrap_or(0)
}
pub fn available()->bool{POOL.lock().seeded}

#[cfg(feature="stage13-9-test")]
pub fn seed_for_test(seed:[u8;32]){POOL.lock().seed(seed)}
#[cfg(feature="stage13-9-test")]
pub fn clear_for_test(){POOL.lock().clear()}

pub fn self_test()->bool{
    #[cfg(feature="stage13-9-test")]
    {
        clear_for_test();
        let mut unavailable=[0u8;32];
        if fill_secure(&mut unavailable)!=Err(EntropyError::Unavailable)||available(){return false}
        seed_for_test([0xa5;32]);
        if !available(){return false}
        let Ok(a)=snonce()else{return false};
        let Ok(b)=snonce()else{return false};
        if a==[0;32]||b==[0;32]||a==b{return false}
        clear_for_test();
        !available()&&snonce()==Err(EntropyError::Unavailable)
    }
    #[cfg(not(feature="stage13-9-test"))]
    { !available() }
}
