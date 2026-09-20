//! WovenWiFi Stage 13.9N — WPA2 security lifecycle closure.
//!
//! Coordinates live Message-3 GTK processing and association teardown around
//! the already-accepted Stage G/H primitives without inventing entropy.
//! Production SNonce must come from a future reviewed kernel CSPRNG.

use crate::wifi_crypto::Ptk;
use crate::wifi_gtk::{self, GroupKeyStore, GtkError, InstallOutcome};
use crate::wifi_rsn::{self, EapolError};
use zeroize::Zeroize;

#[derive(Debug,PartialEq,Eq)]
pub enum SecurityError{
    Protocol(EapolError),
    Gtk(GtkError),
    MissingEncryptedKeyData,
    WrongReplay,
    WrongNonce,
    NoPairwiseKey,
}
impl From<EapolError> for SecurityError{fn from(v:EapolError)->Self{Self::Protocol(v)}}
impl From<GtkError> for SecurityError{fn from(v:GtkError)->Self{Self::Gtk(v)}}

pub struct SecurityContext{
    ptk:Option<Ptk>,
    anonce:[u8;32],
    replay:Option<u64>,
    group:GroupKeyStore,
}
impl SecurityContext{
    pub const fn new()->Self{Self{ptk:None,anonce:[0;32],replay:None,group:GroupKeyStore::new()}}
    pub fn install_pairwise(&mut self,ptk:Ptk,anonce:[u8;32]){
        self.clear();self.ptk=Some(ptk);self.anonce=anonce;
    }
    pub fn process_verified_message3(&mut self,message3:&[u8])->Result<InstallOutcome,SecurityError>{
        let key=wifi_rsn::parse_eapol_key(message3)?;
        let Some(ptk)=self.ptk.as_ref()else{return Err(SecurityError::NoPairwiseKey)};
        if key.nonce!=self.anonce{return Err(SecurityError::WrongNonce)}
        if let Some(last)=self.replay{
            if key.replay_counter<last{return Err(SecurityError::WrongReplay)}
        }
        if !key.key_info.encrypted_key_data()||key.key_data.is_empty(){return Err(SecurityError::MissingEncryptedKeyData)}
        let gtk=wifi_gtk::unwrap_gtk(ptk.kek(),key.key_data)?;
        let outcome=self.group.install_verified(key.replay_counter,gtk)?;
        self.replay=Some(key.replay_counter);
        Ok(outcome)
    }
    pub fn temporal_key(&self)->Option<&[u8]>{self.ptk.as_ref().map(Ptk::tk)}
    pub fn gtk_installed(&self)->bool{self.group.gtk().is_some()}
    pub fn gtk_install_count(&self)->u64{self.group.install_count()}
    #[cfg(feature = "stage13-9-test")]
    pub fn install_verified_gtk_for_test(&mut self,replay:u64,index:u8,key:[u8;wifi_gtk::GTK_LEN])->Result<InstallOutcome,SecurityError>{
        let candidate=wifi_gtk::GroupTemporalKey::from_test_bytes(index,key);
        let outcome=self.group.install_verified(replay,candidate)?;
        self.replay=Some(replay);
        Ok(outcome)
    }
    pub fn clear(&mut self){
        self.ptk=None;self.group.clear();self.anonce.zeroize();self.replay=None;
    }
}
impl Drop for SecurityContext{fn drop(&mut self){self.clear();}}

pub fn self_test()->bool{
    let mut bytes=[0u8;crate::wifi_crypto::PTK_LEN];
    bytes[32..48].copy_from_slice(&[0x44;16]);
    let ptk=crate::wifi_crypto::Ptk::from_test_bytes(bytes);
    let mut ctx=SecurityContext::new();
    ctx.install_pairwise(ptk,[0x11;32]);
    if ctx.temporal_key()!=Some(&[0x44;16][..]){return false}
    let gtk=[0x55;wifi_gtk::GTK_LEN];
    if ctx.install_verified_gtk_for_test(7,1,gtk)!=Ok(InstallOutcome::Installed){return false}
    if ctx.install_verified_gtk_for_test(7,1,gtk)!=Ok(InstallOutcome::Retransmission){return false}
    if ctx.gtk_install_count()!=1||!ctx.gtk_installed(){return false}
    if ctx.install_verified_gtk_for_test(7,1,[0x56;wifi_gtk::GTK_LEN])!=Err(SecurityError::Gtk(GtkError::ConflictingRetransmission)){return false}
    ctx.clear();
    ctx.temporal_key().is_none()&&!ctx.gtk_installed()
}
