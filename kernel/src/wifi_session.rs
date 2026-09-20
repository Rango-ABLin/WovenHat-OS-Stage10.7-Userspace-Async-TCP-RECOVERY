//! WovenWiFi Stage 13.9M — cross-module integration closure.
use crate::wifi_backend::{BackendError,VirtualBackend,WifiBackend,MAX_80211_FRAME};
use crate::wifi_net::{BridgeError,WifiNetBridge};
use crate::wifi_recovery::{RecoveryController,RecoveryError,RecoveryState};

#[derive(Debug,PartialEq,Eq)]
pub enum SessionError{Recovery(RecoveryError),Backend(BackendError),Bridge(BridgeError),NoKeys,StaleEpoch,WrongState}
impl From<RecoveryError> for SessionError{fn from(v:RecoveryError)->Self{Self::Recovery(v)}}
impl From<BackendError> for SessionError{fn from(v:BackendError)->Self{Self::Backend(v)}}
impl From<BridgeError> for SessionError{fn from(v:BridgeError)->Self{Self::Bridge(v)}}

pub struct WifiSession{recovery:RecoveryController,backend:VirtualBackend,bridge:Option<WifiNetBridge>,active_epoch:Option<u32>}
impl WifiSession{
 pub const fn new(max_attempts:u8)->Self{Self{recovery:RecoveryController::new(max_attempts),backend:VirtualBackend::new(),bridge:None,active_epoch:None}}
 pub fn begin_connect(&mut self,now:u64,timeout:u64)->Result<u32,SessionError>{self.clear_association();Ok(self.recovery.begin_connect(now,timeout)?)}
 pub fn install_pairwise(&mut self,epoch:u32,station:[u8;6],bssid:[u8;6],tk:&[u8])->Result<(),SessionError>{
  if epoch!=self.recovery.epoch(){return Err(SessionError::StaleEpoch)}
  if self.recovery.state()!=RecoveryState::Connecting{return Err(SessionError::WrongState)}
  self.bridge=Some(WifiNetBridge::new(station,bssid,tk)?);self.backend.set_up(true);self.recovery.connected(epoch)?;self.active_epoch=Some(epoch);Ok(())
 }
 pub fn transmit_ethernet(&mut self,epoch:u32,ethernet:&[u8])->Result<usize,SessionError>{
  self.require_active(epoch)?;let bridge=self.bridge.as_mut().ok_or(SessionError::NoKeys)?;let mut frame=[0u8;MAX_80211_FRAME];
  let n=bridge.encapsulate(ethernet,&mut frame)?;self.backend.transmit(&frame[..n])?;Ok(n)
 }
 pub fn poll_timeout(&mut self,now:u64)->bool{if self.recovery.poll_timeout(now){self.clear_association();true}else{false}}
 pub fn connection_failed(&mut self,epoch:u32)->Result<(),SessionError>{self.recovery.connection_failed(epoch)?;self.clear_association();Ok(())}
 pub fn disconnect(&mut self)->u32{let e=self.recovery.disconnect();self.clear_association();e}
 pub fn dequeue_tx(&mut self,out:&mut[u8])->Result<Option<usize>,SessionError>{Ok(self.backend.dequeue_tx(out)?)}
 pub const fn has_pairwise_keys(&self)->bool{self.bridge.is_some()}
 fn require_active(&self,epoch:u32)->Result<(),SessionError>{if self.active_epoch!=Some(epoch)||self.recovery.epoch()!=epoch{return Err(SessionError::StaleEpoch)}
  if self.recovery.state()!=RecoveryState::Connected{return Err(SessionError::WrongState)}Ok(())}
 fn clear_association(&mut self){self.bridge=None;self.active_epoch=None;self.backend.set_up(false);}
}
pub fn self_test()->bool{
 let sta=[2,0,0,0,0,1];let ap=[2,0,0,0,0,2];let peer=[2,0,0,0,0,3];let mut eth=[0u8;64];
 eth[..6].copy_from_slice(&peer);eth[6..12].copy_from_slice(&sta);eth[12..14].copy_from_slice(&[8,0]);
 let mut s=WifiSession::new(3);let Ok(e1)=s.begin_connect(10,5)else{return false};
 if s.install_pairwise(e1,sta,ap,&[0x11;16]).is_err()||!s.has_pairwise_keys(){return false}
 if s.transmit_ethernet(e1,&eth).is_err(){return false}let mut out=[0u8;MAX_80211_FRAME];if !matches!(s.dequeue_tx(&mut out),Ok(Some(_))){return false}
 let e2=s.disconnect();if e2==e1||s.has_pairwise_keys()||s.transmit_ethernet(e1,&eth)!=Err(SessionError::StaleEpoch){return false}
 let Ok(e3)=s.begin_connect(20,2)else{return false};if e3!=e2||!s.poll_timeout(22)||s.has_pairwise_keys(){return false}
 if s.install_pairwise(e3,sta,ap,&[0x22;16])!=Err(SessionError::StaleEpoch){return false}true
}
