//! WovenWiFi Stage 13.9J — WovenNet integration boundary.
//!
//! Bridges Ethernet frames used by WovenNet/smoltcp to protected 802.11 data
//! frames.  Outbound traffic is encapsulated as RFC 1042 LLC/SNAP then passed
//! through Stage 13.9I CCMP.  Inbound traffic is admitted only after CCMP
//! authentication/replay checks and valid LLC/SNAP decapsulation.
//!
//! This is transport-neutral: a physical Wi-Fi driver will submit/receive the
//! resulting 802.11 frames in Stage 13.9K+.

use crate::wifi80211::DATA_HEADER_LEN;
use crate::wifi_ccmp::{self, CcmpError, RxState, TemporalKey, TxState};
use zeroize::Zeroize;

const ETH_HEADER_LEN: usize = 14;
const LLC_SNAP_LEN: usize = 8;
const MAX_ETH_FRAME: usize = 1514;
const MAX_WIFI_FRAME: usize = DATA_HEADER_LEN + 8 + (MAX_ETH_FRAME - ETH_HEADER_LEN + LLC_SNAP_LEN) + 8;
const RFC1042_PREFIX: [u8; 6] = [0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeError {
    EthernetFrame,
    WrongPeer,
    Direction,
    LlcSnap,
    BufferTooSmall,
    Ccmp(CcmpError),
}
impl From<CcmpError> for BridgeError { fn from(value:CcmpError)->Self{Self::Ccmp(value)} }

pub struct WifiNetBridge {
    station:[u8;6],
    bssid:[u8;6],
    pairwise:TemporalKey,
    tx:TxState,
    rx:RxState,
    group:Option<(u8,TemporalKey,RxState)>,
}
impl WifiNetBridge {
    pub fn new(station:[u8;6],bssid:[u8;6],tk:&[u8])->Result<Self,BridgeError>{
        Ok(Self{station,bssid,pairwise:TemporalKey::new(tk)?,tx:TxState::new(),rx:RxState::new(),group:None})
    }
    pub const fn tx_packet_number(&self)->u64{self.tx.packet_number()}
    pub const fn rx_packet_number(&self)->u64{self.rx.last_packet_number()}
    pub fn install_group_key(&mut self,index:u8,gtk:&[u8])->Result<(),BridgeError>{let next=TemporalKey::new(gtk)?;self.group=Some((index&3,next,RxState::new()));Ok(())}
    pub fn group_rx_packet_number(&self)->u64{self.group.as_ref().map_or(0,|(_,_,rx)|rx.last_packet_number())}

    /// Convert one WovenNet Ethernet-II frame to a protected station->DS frame.
    pub fn encapsulate(&mut self,ethernet:&[u8],out:&mut[u8])->Result<usize,BridgeError>{
        if ethernet.len()<ETH_HEADER_LEN||ethernet.len()>MAX_ETH_FRAME{return Err(BridgeError::EthernetFrame)}
        let payload_len=LLC_SNAP_LEN+ethernet.len()-ETH_HEADER_LEN;
        let mut payload=[0u8;MAX_ETH_FRAME-ETH_HEADER_LEN+LLC_SNAP_LEN];
        payload[..6].copy_from_slice(&RFC1042_PREFIX);
        payload[6..8].copy_from_slice(&ethernet[12..14]);
        payload[8..payload_len].copy_from_slice(&ethernet[14..]);

        let mut header=[0u8;DATA_HEADER_LEN];
        // Data + ToDS. CCMP sets Protected in the emitted copy.
        header[0]=0x08; header[1]=0x01;
        header[4..10].copy_from_slice(&self.bssid);
        header[10..16].copy_from_slice(&self.station);
        header[16..22].copy_from_slice(&ethernet[..6]);
        let result=wifi_ccmp::protect(&self.pairwise,&mut self.tx,&header,&payload[..payload_len],0,out);
        payload.zeroize();
        result.map_err(BridgeError::from)
    }

    /// Authenticate/decrypt one AP->station frame and reconstruct Ethernet-II.
    pub fn decapsulate(&mut self,frame:&[u8],out:&mut[u8])->Result<usize,BridgeError>{
        if frame.len()<DATA_HEADER_LEN{return Err(BridgeError::EthernetFrame)}
        let h=&frame[..DATA_HEADER_LEN];
        // FromDS=1, ToDS=0 for infrastructure downlink.
        if h[1]&0x03!=0x02{return Err(BridgeError::Direction)}
        let group_address=h[4]&1!=0;if (!group_address&&h[4..10]!=self.station)||h[10..16]!=self.bssid{return Err(BridgeError::WrongPeer)}
        let mut plain=[0u8;MAX_ETH_FRAME-ETH_HEADER_LEN+LLC_SNAP_LEN];
        
        let ccmp=&frame[DATA_HEADER_LEN..];
        let key_id=(ccmp.get(3).copied().ok_or(BridgeError::EthernetFrame)?>>6)&3;
        let result=if group_address{
            let Some((installed_id,key,rx))=self.group.as_mut() else{
                plain.zeroize();return Err(BridgeError::Ccmp(CcmpError::InvalidKey));
            };
            if key_id!=*installed_id{plain.zeroize();return Err(BridgeError::Ccmp(CcmpError::InvalidKey));}
            wifi_ccmp::unprotect(key,rx,frame,&mut plain)
        }else{
            if key_id!=0{plain.zeroize();return Err(BridgeError::Ccmp(CcmpError::InvalidKey));}
            wifi_ccmp::unprotect(&self.pairwise,&mut self.rx,frame,&mut plain)
        };
        let len=match result{Ok(n)=>n,Err(e)=>{plain.zeroize();return Err(BridgeError::Ccmp(e));}};
        if len<LLC_SNAP_LEN||plain[..6]!=RFC1042_PREFIX{plain.zeroize();return Err(BridgeError::LlcSnap)}
        let total=ETH_HEADER_LEN+len-LLC_SNAP_LEN;
        if total>MAX_ETH_FRAME||out.len()<total{plain.zeroize();return Err(BridgeError::BufferTooSmall)}
        out[..6].copy_from_slice(&h[4..10]);
        out[6..12].copy_from_slice(&h[16..22]);
        out[12..14].copy_from_slice(&plain[6..8]);
        out[14..total].copy_from_slice(&plain[8..len]);
        plain.zeroize();
        Ok(total)
    }
}

fn make_downlink(key:&TemporalKey,tx:&mut TxState,station:[u8;6],bssid:[u8;6],source:[u8;6],ethernet:&[u8],out:&mut[u8])->Result<usize,CcmpError>{
    let mut h=[0u8;24];h[0]=0x08;h[1]=0x02;h[4..10].copy_from_slice(&station);h[10..16].copy_from_slice(&bssid);h[16..22].copy_from_slice(&source);
    let mut p=[0u8;1508];p[..6].copy_from_slice(&RFC1042_PREFIX);p[6..8].copy_from_slice(&ethernet[12..14]);let n=8+ethernet.len()-14;p[8..n].copy_from_slice(&ethernet[14..]);let r=wifi_ccmp::protect(key,tx,&h,&p[..n],0,out);p.zeroize();r
}

#[cfg(feature="stage13-9-test")]
pub fn group_ccmp_self_test()->bool{
    let sta=[0x02,0,0,0,0,1];let ap=[0x02,0,0,0,0,2];let group=[0x01,0,0x5e,0,0,1];let peer=[0x02,0,0,0,0,3];
    let ptk=[0x44;16];let gtk=[0x5a;16];let key_id=1u8;
    let mut bridge=match WifiNetBridge::new(sta,ap,&ptk){Ok(v)=>v,Err(_)=>return false};
    if bridge.install_group_key(key_id,&gtk).is_err(){return false}
    let key=match TemporalKey::new(&gtk){Ok(v)=>v,Err(_)=>return false};let mut tx=TxState::new();
    let mut h=[0u8;24];h[0]=0x08;h[1]=0x02;h[4..10].copy_from_slice(&group);h[10..16].copy_from_slice(&ap);h[16..22].copy_from_slice(&peer);
    let mut eth=[0u8;64];eth[..6].copy_from_slice(&group);eth[6..12].copy_from_slice(&peer);eth[12..14].copy_from_slice(&[0x08,0x00]);
    let mut p=[0u8;1508];p[..6].copy_from_slice(&RFC1042_PREFIX);p[6..8].copy_from_slice(&eth[12..14]);p[8..58].copy_from_slice(&eth[14..64]);
    let mut frame=[0u8;MAX_WIFI_FRAME];let Ok(n)=wifi_ccmp::protect(&key,&mut tx,&h,&p[..58],key_id,&mut frame)else{return false};
    let mut out=[0u8;MAX_ETH_FRAME];let Ok(rn)=bridge.decapsulate(&frame[..n],&mut out)else{return false};
    if out[..rn]!=eth||bridge.group_rx_packet_number()!=1||bridge.rx_packet_number()!=0{return false}
    if bridge.decapsulate(&frame[..n],&mut out)!=Err(BridgeError::Ccmp(CcmpError::Replay)){return false}
    let mut wrong=frame;wrong[DATA_HEADER_LEN+3]=(wrong[DATA_HEADER_LEN+3]&0x3f)|(2<<6);
    if bridge.decapsulate(&wrong[..n],&mut out)!=Err(BridgeError::Ccmp(CcmpError::InvalidKey)){return false}
    let Ok(n2)=wifi_ccmp::protect(&key,&mut tx,&h,&p[..58],key_id,&mut frame)else{return false};
    let mut bad=frame;bad[n2-1]^=1;
    if bridge.decapsulate(&bad[..n2],&mut out)!=Err(BridgeError::Ccmp(CcmpError::Authentication))||bridge.group_rx_packet_number()!=1{return false}
    bridge.decapsulate(&frame[..n2],&mut out).is_ok()&&bridge.group_rx_packet_number()==2
}
pub fn self_test()->bool{
    let sta=[0x02,0,0,0,0,1];let ap=[0x02,0,0,0,0,2];let peer=[0x02,0,0,0,0,3];let tk=[0x44;16];
    let mut eth=[0u8;64];eth[..6].copy_from_slice(&peer);eth[6..12].copy_from_slice(&sta);eth[12..14].copy_from_slice(&[0x08,0x00]);for(i,b)in eth[14..].iter_mut().enumerate(){*b=i as u8;}
    let mut bridge=match WifiNetBridge::new(sta,ap,&tk){Ok(v)=>v,Err(_)=>return false};
    let mut wifi=[0u8;MAX_WIFI_FRAME];let Ok(_n)=bridge.encapsulate(&eth,&mut wifi)else{return false};
    if bridge.tx_packet_number()!=1||wifi[1]&0x43!=0x41||wifi[4..10]!=ap||wifi[10..16]!=sta||wifi[16..22]!=peer{return false}

    let key=match TemporalKey::new(&tk){Ok(v)=>v,Err(_)=>return false};let mut ap_tx=TxState::new();let mut down=[0u8;MAX_WIFI_FRAME];
    // Downlink Ethernet destination is station, source is peer.
    let mut inbound=eth;inbound[..6].copy_from_slice(&sta);inbound[6..12].copy_from_slice(&peer);
    let Ok(dn)=make_downlink(&key,&mut ap_tx,sta,ap,peer,&inbound,&mut down)else{return false};
    let mut recovered=[0u8;MAX_ETH_FRAME];let Ok(rn)=bridge.decapsulate(&down[..dn],&mut recovered)else{return false};
    if recovered[..rn]!=inbound||bridge.rx_packet_number()!=1{return false}
    // Authenticated replay must never reach WovenNet.
    if bridge.decapsulate(&down[..dn],&mut recovered)!=Err(BridgeError::Ccmp(CcmpError::Replay)){return false}
    // A forged frame from another BSSID is rejected before decryption.
    let mut forged=down;forged[10]^=1;if bridge.decapsulate(&forged[..dn],&mut recovered)!=Err(BridgeError::WrongPeer){return false}
    // Tamper a fresh protected frame: authentication failure and RX PN unchanged.
    let Ok(dn2)=make_downlink(&key,&mut ap_tx,sta,ap,peer,&inbound,&mut down)else{return false};down[dn2-1]^=1;
    if bridge.decapsulate(&down[..dn2],&mut recovered)!=Err(BridgeError::Ccmp(CcmpError::Authentication))||bridge.rx_packet_number()!=1{return false}
    true
}
