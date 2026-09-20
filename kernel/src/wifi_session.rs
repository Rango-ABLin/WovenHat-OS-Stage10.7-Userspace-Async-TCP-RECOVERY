//! WovenWiFi Stage 13.9T — backend RX -> CCMP -> Ethernet session integration.
//!
//! Completes the virtual receive path so backend-delivered protected 802.11
//! frames are authenticated/decrypted by the existing CCMP bridge before being
//! surfaced as Ethernet frames to WovenNet-facing callers.

use crate::wifi_backend::{BackendError,VirtualBackend,WifiBackend,MAX_80211_FRAME};
use crate::wifi_net::{BridgeError,WifiNetBridge};
use crate::wifi_recovery::{RecoveryController,RecoveryError,RecoveryState};
use crate::wifi_wpa2::{SupplicantState,Wpa2Supplicant};

#[derive(Debug,PartialEq,Eq)]
pub enum SessionError{
    Recovery(RecoveryError),Backend(BackendError),Bridge(BridgeError),
    NoKeys,StaleEpoch,WrongState,HandshakeIncomplete,MissingGtk,
}
impl From<RecoveryError> for SessionError{fn from(v:RecoveryError)->Self{Self::Recovery(v)}}
impl From<BackendError> for SessionError{fn from(v:BackendError)->Self{Self::Backend(v)}}
impl From<BridgeError> for SessionError{fn from(v:BridgeError)->Self{Self::Bridge(v)}}

pub struct WifiSession{
    recovery:RecoveryController,
    backend:VirtualBackend,
    bridge:Option<WifiNetBridge>,
    active_epoch:Option<u32>,
}
impl WifiSession{
    pub const fn new(max_attempts:u8)->Self{
        Self{recovery:RecoveryController::new(max_attempts),backend:VirtualBackend::new(),bridge:None,active_epoch:None}
    }

    pub fn begin_connect(&mut self,now:u64,timeout:u64)->Result<u32,SessionError>{
        self.clear_association();
        Ok(self.recovery.begin_connect(now,timeout)?)
    }

    pub fn install_pairwise(&mut self,epoch:u32,station:[u8;6],bssid:[u8;6],tk:&[u8])->Result<(),SessionError>{
        if epoch!=self.recovery.epoch(){return Err(SessionError::StaleEpoch)}
        if self.recovery.state()!=RecoveryState::Connecting{return Err(SessionError::WrongState)}
        self.activate_bridge(epoch,station,bssid,tk)
    }

    pub fn activate_from_wpa2(
        &mut self,
        epoch:u32,
        station:[u8;6],
        bssid:[u8;6],
        supplicant:&Wpa2Supplicant,
    )->Result<(),SessionError>{
        if epoch!=self.recovery.epoch(){return Err(SessionError::StaleEpoch)}
        if self.recovery.state()!=RecoveryState::Connecting{return Err(SessionError::WrongState)}
        if supplicant.state()!=SupplicantState::Completed{return Err(SessionError::HandshakeIncomplete)}
        if !supplicant.gtk_installed(){return Err(SessionError::MissingGtk)}
        let tk=supplicant.temporal_key().ok_or(SessionError::NoKeys)?;
        self.activate_bridge(epoch,station,bssid,tk)
    }

    fn activate_bridge(&mut self,epoch:u32,station:[u8;6],bssid:[u8;6],tk:&[u8])->Result<(),SessionError>{
        self.bridge=Some(WifiNetBridge::new(station,bssid,tk)?);
        self.backend.set_up(true);
        self.recovery.connected(epoch)?;
        self.active_epoch=Some(epoch);
        Ok(())
    }

    pub fn transmit_ethernet(&mut self,epoch:u32,ethernet:&[u8])->Result<usize,SessionError>{
        self.require_active(epoch)?;
        let bridge=self.bridge.as_mut().ok_or(SessionError::NoKeys)?;
        let mut frame=[0u8;MAX_80211_FRAME];
        let n=bridge.encapsulate(ethernet,&mut frame)?;
        self.backend.transmit(&frame[..n])?;
        Ok(n)
    }

    /// Pull one backend RX frame through peer/direction checks, CCMP replay and
    /// authentication, then LLC/SNAP decapsulation into Ethernet-II.
    pub fn receive_ethernet(&mut self,epoch:u32,out:&mut[u8])->Result<Option<usize>,SessionError>{
        self.require_active(epoch)?;
        let mut frame=[0u8;MAX_80211_FRAME];
        let Some(n)=self.backend.receive_into(&mut frame)? else{return Ok(None)};
        let bridge=self.bridge.as_mut().ok_or(SessionError::NoKeys)?;
        Ok(Some(bridge.decapsulate(&frame[..n],out)?))
    }

    pub fn poll_timeout(&mut self,now:u64)->bool{
        if self.recovery.poll_timeout(now){self.clear_association();true}else{false}
    }

    pub fn connection_failed(&mut self,epoch:u32)->Result<(),SessionError>{
        self.recovery.connection_failed(epoch)?;
        self.clear_association();
        Ok(())
    }

    pub fn disconnect(&mut self)->u32{
        let e=self.recovery.disconnect();
        self.clear_association();
        e
    }

    pub fn dequeue_tx(&mut self,out:&mut[u8])->Result<Option<usize>,SessionError>{
        Ok(self.backend.dequeue_tx(out)?)
    }

    #[cfg(feature="stage13-9-test")]
    pub fn inject_rx_for_test(&mut self,frame:&[u8])->Result<(),SessionError>{
        Ok(self.backend.inject_rx(frame)?)
    }

    pub const fn has_pairwise_keys(&self)->bool{self.bridge.is_some()}

    pub fn is_active_epoch(&self,epoch:u32)->bool{
        self.active_epoch==Some(epoch)
            && self.recovery.epoch()==epoch
            && self.recovery.state()==RecoveryState::Connected
            && self.bridge.is_some()
    }

    fn require_active(&self,epoch:u32)->Result<(),SessionError>{
        if self.active_epoch!=Some(epoch)||self.recovery.epoch()!=epoch{return Err(SessionError::StaleEpoch)}
        if self.recovery.state()!=RecoveryState::Connected{return Err(SessionError::WrongState)}
        Ok(())
    }

    fn clear_association(&mut self){
        self.bridge=None;
        self.active_epoch=None;
        self.backend.set_up(false);
    }
}

pub fn self_test()->bool{
    let sta=[2,0,0,0,0,1];
    let ap=[2,0,0,0,0,2];
    let peer=[2,0,0,0,0,3];
    let mut eth=[0u8;64];
    eth[..6].copy_from_slice(&peer);
    eth[6..12].copy_from_slice(&sta);
    eth[12..14].copy_from_slice(&[8,0]);

    let mut s=WifiSession::new(3);
    let Ok(e1)=s.begin_connect(10,5)else{return false};
    if s.install_pairwise(e1,sta,ap,&[0x11;16]).is_err()||!s.has_pairwise_keys(){return false}
    if s.transmit_ethernet(e1,&eth).is_err(){return false}
    let mut out=[0u8;MAX_80211_FRAME];
    if !matches!(s.dequeue_tx(&mut out),Ok(Some(_))){return false}

    let e2=s.disconnect();
    if e2==e1||s.has_pairwise_keys()||s.transmit_ethernet(e1,&eth)!=Err(SessionError::StaleEpoch){return false}

    let Ok(e3)=s.begin_connect(20,2)else{return false};
    if e3!=e2||!s.poll_timeout(22)||s.has_pairwise_keys(){return false}
    if s.install_pairwise(e3,sta,ap,&[0x22;16])!=Err(SessionError::StaleEpoch){return false}
    true
}

#[cfg(feature="stage13-9-test")]
pub fn wpa2_handoff_self_test()->bool{
    use crate::wifi_crypto;
    use crate::wifi_gtk;
    use crate::wifi_rsn;
    use crate::wifi_wpa2::Wpa2Supplicant;

    const EAPOL_LEN:usize=4+wifi_rsn::EAPOL_KEY_FIXED_LEN;
    const PAIRWISE:u16=1<<3;
    const INSTALL:u16=1<<6;
    const ACK:u16=1<<7;
    const MIC:u16=1<<8;
    const SECURE:u16=1<<9;
    const ENCRYPTED:u16=1<<12;
    const DESC:u16=2;

    fn build(out:&mut[u8],info:u16,replay:u64,nonce:[u8;32],data:&[u8])->Option<usize>{
        let body=wifi_rsn::EAPOL_KEY_FIXED_LEN.checked_add(data.len())?;
        let total=4usize.checked_add(body)?;
        if out.len()<total||body>u16::MAX as usize||data.len()>u16::MAX as usize{return None}
        out[..total].fill(0);
        out[0]=2;out[1]=wifi_rsn::EAPOL_TYPE_KEY;
        out[2..4].copy_from_slice(&(body as u16).to_be_bytes());
        out[4]=wifi_rsn::EAPOL_KEY_DESCRIPTOR_RSN;
        out[5..7].copy_from_slice(&info.to_be_bytes());
        out[9..17].copy_from_slice(&replay.to_be_bytes());
        out[17..49].copy_from_slice(&nonce);
        out[97..99].copy_from_slice(&(data.len() as u16).to_be_bytes());
        out[99..total].copy_from_slice(data);
        Some(total)
    }

    let rsn=[1,0,0,0x0f,0xac,4,1,0,0,0x0f,0xac,4,1,0,0,0x0f,0xac,2,0,0];
    let Ok(profile)=wifi_rsn::parse_rsn(&rsn)else{return false};
    let Ok(pmk)=wifi_crypto::derive_pmk(b"password",b"IEEE")else{return false};

    let station=[0x00,0x13,0x46,0xfe,0x32,0x0c];
    let ap=[0x00,0x14,0x6c,0x7e,0x40,0x80];
    let snonce=[0x22;32];
    let anonce=[0x11;32];

    let mut session=WifiSession::new(3);
    let Ok(epoch)=session.begin_connect(100,20)else{return false};

    let mut supplicant=Wpa2Supplicant::new(station);
    if supplicant.begin(profile,ap,snonce).is_err(){return false}
    if session.activate_from_wpa2(epoch,station,ap,&supplicant)!=Err(SessionError::HandshakeIncomplete){return false}

    let mut msg1=[0u8;EAPOL_LEN];
    let Some(n1)=build(&mut msg1,PAIRWISE|ACK|DESC,1,anonce,&[])else{return false};
    let mut msg2=[0u8;EAPOL_LEN];
    if supplicant.receive_message1_build_message2(&pmk,&msg1[..n1],&mut msg2).is_err(){return false}

    let Ok(ap_ptk)=wifi_crypto::derive_ptk(&pmk,ap,station,anonce,snonce)else{return false};
    let mut wrapped=[0u8;40];
    let Ok(wn)=wifi_gtk::wrap_gtk_for_test(ap_ptk.kek(),1,[0x5a;wifi_gtk::GTK_LEN],&mut wrapped)else{return false};

    let mut msg3=[0u8;160];
    let Some(n3)=build(&mut msg3,PAIRWISE|INSTALL|ACK|MIC|SECURE|ENCRYPTED|DESC,2,anonce,&wrapped[..wn])else{return false};
    let Ok(mic)=wifi_crypto::compute_eapol_mic(ap_ptk.kck(),&msg3[..n3])else{return false};
    msg3[81..97].copy_from_slice(&mic);

    let mut msg4=[0u8;EAPOL_LEN];
    if supplicant.receive_message3_build_message4(&msg3[..n3],&mut msg4).is_err(){return false}
    if supplicant.gtk_index()!=Some(1)||supplicant.gtk_install_count()!=1{return false}

    if session.activate_from_wpa2(epoch,station,ap,&supplicant).is_err(){return false}
    if !session.has_pairwise_keys(){return false}
    if session.activate_from_wpa2(epoch,station,ap,&supplicant)!=Err(SessionError::WrongState){return false}

    let peer=[2,0,0,0,0,3];
    let mut eth=[0u8;64];
    eth[..6].copy_from_slice(&peer);
    eth[6..12].copy_from_slice(&station);
    eth[12..14].copy_from_slice(&[0x08,0x00]);
    if session.transmit_ethernet(epoch,&eth).is_err(){return false}

    let next=session.disconnect();
    next!=epoch&&!session.has_pairwise_keys()
}

#[cfg(feature="stage13-9-test")]
pub fn rx_path_self_test()->bool{
    use crate::wifi_ccmp::{self,TemporalKey,TxState};

    const RFC1042:[u8;6]=[0xaa,0xaa,0x03,0x00,0x00,0x00];

    fn make_downlink(
        key:&TemporalKey,
        tx:&mut TxState,
        station:[u8;6],
        bssid:[u8;6],
        source:[u8;6],
        ethernet:&[u8],
        out:&mut[u8],
    )->Option<usize>{
        if ethernet.len()<14{return None}
        let mut h=[0u8;24];
        h[0]=0x08;h[1]=0x02;
        h[4..10].copy_from_slice(&station);
        h[10..16].copy_from_slice(&bssid);
        h[16..22].copy_from_slice(&source);

        let mut payload=[0u8;1508];
        payload[..6].copy_from_slice(&RFC1042);
        payload[6..8].copy_from_slice(&ethernet[12..14]);
        let n=8+ethernet.len()-14;
        payload[8..n].copy_from_slice(&ethernet[14..]);
        wifi_ccmp::protect(key,tx,&h,&payload[..n],0,out).ok()
    }

    let station=[0x02,0,0,0,0,1];
    let ap=[0x02,0,0,0,0,2];
    let peer=[0x02,0,0,0,0,3];
    let tk=[0x44;16];

    let mut session=WifiSession::new(2);
    let Ok(epoch)=session.begin_connect(1,10)else{return false};
    if session.install_pairwise(epoch,station,ap,&tk).is_err(){return false}

    let mut ethernet=[0u8;64];
    ethernet[..6].copy_from_slice(&station);
    ethernet[6..12].copy_from_slice(&peer);
    ethernet[12..14].copy_from_slice(&[0x08,0x00]);
    for(i,b)in ethernet[14..].iter_mut().enumerate(){*b=i as u8}

    let Ok(key)=TemporalKey::new(&tk)else{return false};
    let mut ap_tx=TxState::new();

    let mut frame=[0u8;MAX_80211_FRAME];
    let Some(n)=make_downlink(&key,&mut ap_tx,station,ap,peer,&ethernet,&mut frame)else{return false};
    if session.inject_rx_for_test(&frame[..n]).is_err(){return false}

    let mut recovered=[0u8;1514];
    let Ok(Some(rn))=session.receive_ethernet(epoch,&mut recovered)else{return false};
    if recovered[..rn]!=ethernet{return false}

    // Backend queue is empty after consuming the accepted frame.
    if session.receive_ethernet(epoch,&mut recovered)!=Ok(None){return false}

    // Replay is rejected at the session boundary.
    if session.inject_rx_for_test(&frame[..n]).is_err(){return false}
    if session.receive_ethernet(epoch,&mut recovered)
        !=Err(SessionError::Bridge(BridgeError::Ccmp(wifi_ccmp::CcmpError::Replay)))
    {return false}

    // Tamper a fresh packet: it must fail authentication and must not advance
    // the receive packet-number state, so the untampered copy can still pass.
    let Some(n2)=make_downlink(&key,&mut ap_tx,station,ap,peer,&ethernet,&mut frame)else{return false};
    let mut tampered=frame;
    tampered[n2-1]^=1;
    if session.inject_rx_for_test(&tampered[..n2]).is_err(){return false}
    if session.receive_ethernet(epoch,&mut recovered)
        !=Err(SessionError::Bridge(BridgeError::Ccmp(wifi_ccmp::CcmpError::Authentication)))
    {return false}
    if session.inject_rx_for_test(&frame[..n2]).is_err(){return false}
    let Ok(Some(rn2))=session.receive_ethernet(epoch,&mut recovered)else{return false};
    if recovered[..rn2]!=ethernet{return false}

    let old=epoch;
    let new=session.disconnect();
    if new==old{return false}
    session.receive_ethernet(old,&mut recovered)==Err(SessionError::StaleEpoch)
}
