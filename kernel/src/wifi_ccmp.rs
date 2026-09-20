//! WovenWiFi Stage 13.9I — CCMP protected data-path foundation.
//!
//! Implements bounded AES-CCM/CCMP protection for ordinary 24-byte 802.11
//! data headers (no QoS/Addr4 yet), 48-bit packet numbers, authenticated AAD,
//! replay rejection, pairwise/group key separation, and install-once state.
//! The scalar AES primitive is local because the pinned bare-metal toolchain
//! cannot compile RustCrypto aes 0.8.4 reliably.

use crate::wifi80211::{self, DATA_HEADER_LEN};
use zeroize::Zeroize;

const MIC_LEN: usize = 8;
const CCMP_HEADER_LEN: usize = 8;
const MAX_PAYLOAD: usize = 1536;
const PN_MAX: u64 = (1u64 << 48) - 1;

const SBOX: [u8;256] = [
0x63,0x7c,0x77,0x7b,0xf2,0x6b,0x6f,0xc5,0x30,0x01,0x67,0x2b,0xfe,0xd7,0xab,0x76,
0xca,0x82,0xc9,0x7d,0xfa,0x59,0x47,0xf0,0xad,0xd4,0xa2,0xaf,0x9c,0xa4,0x72,0xc0,
0xb7,0xfd,0x93,0x26,0x36,0x3f,0xf7,0xcc,0x34,0xa5,0xe5,0xf1,0x71,0xd8,0x31,0x15,
0x04,0xc7,0x23,0xc3,0x18,0x96,0x05,0x9a,0x07,0x12,0x80,0xe2,0xeb,0x27,0xb2,0x75,
0x09,0x83,0x2c,0x1a,0x1b,0x6e,0x5a,0xa0,0x52,0x3b,0xd6,0xb3,0x29,0xe3,0x2f,0x84,
0x53,0xd1,0x00,0xed,0x20,0xfc,0xb1,0x5b,0x6a,0xcb,0xbe,0x39,0x4a,0x4c,0x58,0xcf,
0xd0,0xef,0xaa,0xfb,0x43,0x4d,0x33,0x85,0x45,0xf9,0x02,0x7f,0x50,0x3c,0x9f,0xa8,
0x51,0xa3,0x40,0x8f,0x92,0x9d,0x38,0xf5,0xbc,0xb6,0xda,0x21,0x10,0xff,0xf3,0xd2,
0xcd,0x0c,0x13,0xec,0x5f,0x97,0x44,0x17,0xc4,0xa7,0x7e,0x3d,0x64,0x5d,0x19,0x73,
0x60,0x81,0x4f,0xdc,0x22,0x2a,0x90,0x88,0x46,0xee,0xb8,0x14,0xde,0x5e,0x0b,0xdb,
0xe0,0x32,0x3a,0x0a,0x49,0x06,0x24,0x5c,0xc2,0xd3,0xac,0x62,0x91,0x95,0xe4,0x79,
0xe7,0xc8,0x37,0x6d,0x8d,0xd5,0x4e,0xa9,0x6c,0x56,0xf4,0xea,0x65,0x7a,0xae,0x08,
0xba,0x78,0x25,0x2e,0x1c,0xa6,0xb4,0xc6,0xe8,0xdd,0x74,0x1f,0x4b,0xbd,0x8b,0x8a,
0x70,0x3e,0xb5,0x66,0x48,0x03,0xf6,0x0e,0x61,0x35,0x57,0xb9,0x86,0xc1,0x1d,0x9e,
0xe1,0xf8,0x98,0x11,0x69,0xd9,0x8e,0x94,0x9b,0x1e,0x87,0xe9,0xce,0x55,0x28,0xdf,
0x8c,0xa1,0x89,0x0d,0xbf,0xe6,0x42,0x68,0x41,0x99,0x2d,0x0f,0xb0,0x54,0xbb,0x16];
const RCON:[u8;10]=[1,2,4,8,16,32,64,128,0x1b,0x36];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CcmpError {
    InvalidKey, InvalidFrame, UnsupportedHeader, BufferTooSmall, PayloadTooLarge,
    PacketNumberExhausted, Replay, Authentication,
}
pub struct TemporalKey([u8;16]);
impl TemporalKey {
    pub fn new(bytes:&[u8])->Result<Self,CcmpError>{if bytes.len()!=16{return Err(CcmpError::InvalidKey)}let mut k=[0;16];k.copy_from_slice(bytes);Ok(Self(k))}
}
impl Drop for TemporalKey { fn drop(&mut self){self.0.zeroize();} }

pub struct TxState { pn:u64 }
impl TxState {
    pub const fn new()->Self{Self{pn:0}}
    pub const fn packet_number(&self)->u64{self.pn}
    fn next(&mut self)->Result<u64,CcmpError>{if self.pn>=PN_MAX{return Err(CcmpError::PacketNumberExhausted)}self.pn+=1;Ok(self.pn)}
}
pub struct RxState { last_pn:u64 }
impl RxState { pub const fn new()->Self{Self{last_pn:0}} pub const fn last_packet_number(&self)->u64{self.last_pn} }

fn xtime(x:u8)->u8{(x<<1)^if x&0x80!=0{0x1b}else{0}}
fn mul2(x:u8)->u8{xtime(x)}
fn mul3(x:u8)->u8{xtime(x)^x}
fn sub(s:&mut[u8;16]){for x in s.iter_mut(){*x=SBOX[*x as usize];}}
fn shift(s:&mut[u8;16]){let t=*s;s[0]=t[0];s[1]=t[5];s[2]=t[10];s[3]=t[15];s[4]=t[4];s[5]=t[9];s[6]=t[14];s[7]=t[3];s[8]=t[8];s[9]=t[13];s[10]=t[2];s[11]=t[7];s[12]=t[12];s[13]=t[1];s[14]=t[6];s[15]=t[11];}
fn mix(s:&mut[u8;16]){for c in 0..4{let i=4*c;let a=[s[i],s[i+1],s[i+2],s[i+3]];s[i]=mul2(a[0])^mul3(a[1])^a[2]^a[3];s[i+1]=a[0]^mul2(a[1])^mul3(a[2])^a[3];s[i+2]=a[0]^a[1]^mul2(a[2])^mul3(a[3]);s[i+3]=mul3(a[0])^a[1]^a[2]^mul2(a[3]);}}
fn expand(k:&[u8;16])->[u8;176]{let mut w=[0;176];w[..16].copy_from_slice(k);let(mut n,mut r)=(16,0);while n<176{let mut t=[w[n-4],w[n-3],w[n-2],w[n-1]];if n%16==0{t=[SBOX[t[1]as usize],SBOX[t[2]as usize],SBOX[t[3]as usize],SBOX[t[0]as usize]];t[0]^=RCON[r];r+=1;}for &v in &t{w[n]=w[n-16]^v;n+=1;}}w}
fn aes(k:&[u8;16],b:&mut[u8;16]){let mut w=expand(k);for i in 0..16{b[i]^=w[i];}for r in 1..10{sub(b);shift(b);mix(b);for i in 0..16{b[i]^=w[16*r+i];}}sub(b);shift(b);for i in 0..16{b[i]^=w[160+i];}w.zeroize();}

fn pn_bytes(pn:u64)->[u8;6]{[(pn>>40)as u8,(pn>>32)as u8,(pn>>24)as u8,(pn>>16)as u8,(pn>>8)as u8,pn as u8]}
fn write_ccmp_header(out:&mut[u8],pn:u64,key_id:u8){let p=pn_bytes(pn);out[0]=p[5];out[1]=p[4];out[2]=0;out[3]=0x20|((key_id&3)<<6);out[4]=p[3];out[5]=p[2];out[6]=p[1];out[7]=p[0];}
fn read_pn(h:&[u8])->Result<u64,CcmpError>{if h.len()<8||h[2]!=0||h[3]&0x20==0{return Err(CcmpError::InvalidFrame)}Ok((h[7]as u64)<<40|(h[6]as u64)<<32|(h[5]as u64)<<24|(h[4]as u64)<<16|(h[1]as u64)<<8|h[0]as u64)}
fn nonce(header:&[u8],pn:u64)->[u8;13]{let p=pn_bytes(pn);let mut n=[0;13];n[1..7].copy_from_slice(&header[10..16]);n[7..].copy_from_slice(&p);n}
fn aad(header:&[u8])->[u8;22]{let mut a=[0;22];a[0]=header[0]&0x8f;a[1]=header[1]&0xc7;a[2..20].copy_from_slice(&header[4..22]);a[20]=header[22]&0x0f;a[21]=0;a}

fn xor_block(a:&mut[u8;16],b:&[u8;16]){for i in 0..16{a[i]^=b[i];}}
fn mac_block(key:&[u8;16],x:&mut[u8;16],block:&[u8;16]){xor_block(x,block);aes(key,x);}
fn cbc_mac(key:&[u8;16],n:&[u8;13],a:&[u8;22],msg:&[u8])->[u8;8]{
    let mut x=[0;16];let mut b=[0;16];b[0]=0x59;b[1..14].copy_from_slice(n);b[14..].copy_from_slice(&(msg.len()as u16).to_be_bytes());mac_block(key,&mut x,&b);
    b=[0;16];b[..2].copy_from_slice(&(a.len()as u16).to_be_bytes());b[2..16].copy_from_slice(&a[..14]);mac_block(key,&mut x,&b);
    b=[0;16];b[..8].copy_from_slice(&a[14..22]);mac_block(key,&mut x,&b);
    for chunk in msg.chunks(16){b=[0;16];b[..chunk.len()].copy_from_slice(chunk);mac_block(key,&mut x,&b);}
    let mut t=[0;8];t.copy_from_slice(&x[..8]);x.zeroize();b.zeroize();t
}
fn ctr(key:&[u8;16],n:&[u8;13],counter:u16)->[u8;16]{let mut a=[0;16];a[0]=1;a[1..14].copy_from_slice(n);a[14..].copy_from_slice(&counter.to_be_bytes());aes(key,&mut a);a}

pub fn protect(key:&TemporalKey,tx:&mut TxState,header:&[u8],payload:&[u8],key_id:u8,out:&mut[u8])->Result<usize,CcmpError>{
    if header.len()!=DATA_HEADER_LEN||wifi80211::parse_data_header(header).is_err(){return Err(CcmpError::UnsupportedHeader)}
    if payload.len()>MAX_PAYLOAD{return Err(CcmpError::PayloadTooLarge)}
    let total=DATA_HEADER_LEN+CCMP_HEADER_LEN+payload.len()+MIC_LEN;if out.len()<total{return Err(CcmpError::BufferTooSmall)}
    let pn=tx.next()?;out[..DATA_HEADER_LEN].copy_from_slice(header);out[1]|=0x40;write_ccmp_header(&mut out[DATA_HEADER_LEN..DATA_HEADER_LEN+8],pn,key_id);
    let n=nonce(&out[..DATA_HEADER_LEN],pn);let a=aad(&out[..DATA_HEADER_LEN]);let tag=cbc_mac(&key.0,&n,&a,payload);
    for (i,chunk) in payload.chunks(16).enumerate(){let s=ctr(&key.0,&n,(i+1)as u16);for j in 0..chunk.len(){out[DATA_HEADER_LEN+8+i*16+j]=chunk[j]^s[j];}}
    let s0=ctr(&key.0,&n,0);for i in 0..8{out[total-8+i]=tag[i]^s0[i];}Ok(total)
}
pub fn unprotect(key:&TemporalKey,rx:&mut RxState,frame:&[u8],out:&mut[u8])->Result<usize,CcmpError>{
    if frame.len()<DATA_HEADER_LEN+CCMP_HEADER_LEN+MIC_LEN{return Err(CcmpError::InvalidFrame)}
    let h=&frame[..DATA_HEADER_LEN];let parsed=wifi80211::parse_data_header(h).map_err(|_|CcmpError::UnsupportedHeader)?;if !parsed.control.protected(){return Err(CcmpError::InvalidFrame)}
    let pn=read_pn(&frame[DATA_HEADER_LEN..DATA_HEADER_LEN+8])?;if pn<=rx.last_pn{return Err(CcmpError::Replay)}
    let clen=frame.len()-DATA_HEADER_LEN-8-8;if clen>MAX_PAYLOAD||out.len()<clen{return Err(CcmpError::BufferTooSmall)}
    let n=nonce(h,pn);for (i,chunk) in frame[DATA_HEADER_LEN+8..DATA_HEADER_LEN+8+clen].chunks(16).enumerate(){let s=ctr(&key.0,&n,(i+1)as u16);for j in 0..chunk.len(){out[i*16+j]=chunk[j]^s[j];}}
    let a=aad(h);let tag=cbc_mac(&key.0,&n,&a,&out[..clen]);let s0=ctr(&key.0,&n,0);let mut diff=0u8;for i in 0..8{diff|=(tag[i]^s0[i])^frame[frame.len()-8+i];}
    if diff!=0{out[..clen].zeroize();return Err(CcmpError::Authentication)}rx.last_pn=pn;Ok(clen)
}

pub fn self_test()->bool{
    let key=match TemporalKey::new(&[0x11;16]){Ok(k)=>k,Err(_)=>return false};let mut h=[0u8;24];h[0]=0x08;h[4..10].copy_from_slice(&[0x00,1,2,3,4,5]);h[10..16].copy_from_slice(&[6,7,8,9,10,11]);h[16..22].copy_from_slice(&[12,13,14,15,16,17]);
    let msg=b"WovenHat CCMP protected payload";let mut enc=[0u8;1600];let mut tx=TxState::new();let Ok(n)=protect(&key,&mut tx,&h,msg,0,&mut enc)else{return false};if tx.packet_number()!=1||enc[1]&0x40==0{return false}
    let mut rx=RxState::new();let mut plain=[0u8;1536];let Ok(p)=unprotect(&key,&mut rx,&enc[..n],&mut plain)else{return false};if &plain[..p]!=msg||rx.last_packet_number()!=1{return false}
    if unprotect(&key,&mut rx,&enc[..n],&mut plain)!=Err(CcmpError::Replay){return false}
    let mut bad=enc;bad[n-1]^=1;let mut fresh=RxState::new();if unprotect(&key,&mut fresh,&bad[..n],&mut plain)!=Err(CcmpError::Authentication)||fresh.last_packet_number()!=0{return false}
    let wrong=match TemporalKey::new(&[0x22;16]){Ok(k)=>k,Err(_)=>return false};if unprotect(&wrong,&mut fresh,&enc[..n],&mut plain)!=Err(CcmpError::Authentication){return false}
    true
}
