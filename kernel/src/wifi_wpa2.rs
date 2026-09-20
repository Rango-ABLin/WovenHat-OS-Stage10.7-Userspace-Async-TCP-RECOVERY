//! WovenWiFi Stage 13.9G — WPA2 4-way handshake integration.
//!
//! Integrates Stage 13.9E EAPOL parsing/state with Stage 13.9F cryptography.
//! This stage constructs message 2/4 and 4/4, verifies message 3/4 MIC,
//! enforces replay/ANonce checks, and clears key material on failure.
//!
//! Message 3 now owns GTK unwrap/install and KRACK-safe retransmission handling.

use crate::wifi_crypto::{self, CryptoError, Pmk, Ptk};
use crate::wifi_gtk::{self, GroupKeyStore, GtkError, InstallOutcome};
use crate::wifi_rsn::{self, EapolError, FourWayError, FourWayHandshake, FourWayState, RsnProfile};
use zeroize::Zeroize;

const EAPOL_HEADER_LEN: usize = 4;
const KEY_BODY_LEN: usize = wifi_rsn::EAPOL_KEY_FIXED_LEN;
const EAPOL_KEY_FRAME_LEN: usize = EAPOL_HEADER_LEN + KEY_BODY_LEN;
const KEY_INFO_PAIRWISE: u16 = 1 << 3;
const KEY_INFO_MIC: u16 = 1 << 8;
const KEY_INFO_SECURE: u16 = 1 << 9;
const KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupplicantState {
    Idle,
    AwaitingMessage1,
    AwaitingMessage3,
    Completed,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SupplicantError {
    WrongState,
    Protocol(EapolError),
    Handshake(FourWayError),
    Crypto(CryptoError),
    WrongPeer,
    WrongNonce,
    BufferTooSmall,
    UnsupportedDescriptorVersion,
    Entropy(crate::entropy::EntropyError),
    Gtk(GtkError),
}

impl From<EapolError> for SupplicantError {
    fn from(value: EapolError) -> Self { Self::Protocol(value) }
}
impl From<FourWayError> for SupplicantError {
    fn from(value: FourWayError) -> Self { Self::Handshake(value) }
}
impl From<CryptoError> for SupplicantError {
    fn from(value: CryptoError) -> Self { Self::Crypto(value) }
}
impl From<GtkError> for SupplicantError {
    fn from(value: GtkError) -> Self { Self::Gtk(value) }
}

pub struct Wpa2Supplicant {
    state: SupplicantState,
    handshake: FourWayHandshake,
    station: [u8; 6],
    authenticator: [u8; 6],
    snonce: [u8; 32],
    ptk: Option<Ptk>,
    group: GroupKeyStore,
    completed_replay: Option<u64>,
}

impl Wpa2Supplicant {
    pub const fn new(station: [u8; 6]) -> Self {
        Self {
            state: SupplicantState::Idle,
            handshake: FourWayHandshake::new(),
            station,
            authenticator: [0; 6],
            snonce: [0; 32],
            ptk: None,
            group: GroupKeyStore::new(),
            completed_replay: None,
        }
    }

    pub const fn state(&self) -> SupplicantState { self.state }

    pub fn begin(
        &mut self,
        profile: RsnProfile,
        authenticator: [u8; 6],
        snonce: [u8; 32],
    ) -> Result<(), SupplicantError> {
        if self.state != SupplicantState::Idle {
            return Err(SupplicantError::WrongState);
        }
        self.handshake.begin(profile)?;
        self.authenticator = authenticator;
        self.snonce = snonce;
        self.ptk = None;
        self.group.clear();
        self.completed_replay = None;
        self.state = SupplicantState::AwaitingMessage1;
        Ok(())
    }

    pub fn begin_with_secure_nonce(
        &mut self,
        profile: RsnProfile,
        authenticator: [u8; 6],
    ) -> Result<(), SupplicantError> {
        let snonce = crate::entropy::snonce().map_err(SupplicantError::Entropy)?;
        self.begin(profile, authenticator, snonce)
    }
    pub fn receive_message1_build_message2(
        &mut self,
        pmk: &Pmk,
        message1: &[u8],
        output: &mut [u8],
    ) -> Result<usize, SupplicantError> {
        if self.state != SupplicantState::AwaitingMessage1 {
            return Err(SupplicantError::WrongState);
        }
        let key = wifi_rsn::parse_eapol_key(message1)?;
        if key.key_info.descriptor_version() != KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES as u8 {
            self.fail();
            return Err(SupplicantError::UnsupportedDescriptorVersion);
        }
        self.handshake.receive_message1(key)?;
        let anonce = self.handshake.anonce();
        let ptk = wifi_crypto::derive_ptk(
            pmk,
            self.authenticator,
            self.station,
            anonce,
            self.snonce,
        )?;

        let len = build_eapol_key(
            output,
            KEY_INFO_PAIRWISE | KEY_INFO_MIC | KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES,
            key.replay_counter,
            self.snonce,
        )?;
        let mic = wifi_crypto::compute_eapol_mic(ptk.kck(), &output[..len])?;
        output[81..97].copy_from_slice(&mic);

        self.ptk = Some(ptk);
        self.state = SupplicantState::AwaitingMessage3;
        Ok(len)
    }

    pub fn receive_message3_build_message4(
        &mut self,
        message3: &[u8],
        output: &mut [u8],
    ) -> Result<usize, SupplicantError> {
        let key = wifi_rsn::parse_eapol_key(message3)?;
        if key.key_info.descriptor_version() != KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES as u8 {
            self.fail();
            return Err(SupplicantError::UnsupportedDescriptorVersion);
        }
        if self.state == SupplicantState::Completed {
            let Some(ptk) = self.ptk.as_ref() else { return Err(SupplicantError::WrongState); };
            if self.completed_replay != Some(key.replay_counter)
                || key.nonce != self.handshake.anonce()
                || !key.key_info.encrypted_key_data()
                || key.key_data.is_empty()
            { return Err(SupplicantError::WrongState); }
            wifi_crypto::verify_eapol_mic(ptk.kck(), message3)?;
            let gtk = wifi_gtk::unwrap_gtk(ptk.kek(), key.key_data)?;
            if self.group.install_verified(key.replay_counter, gtk)? != InstallOutcome::Retransmission {
                return Err(SupplicantError::WrongState);
            }
            return build_message4(ptk, key.replay_counter, output);
        }
        if self.state != SupplicantState::AwaitingMessage3 { return Err(SupplicantError::WrongState); }
        match self.handshake.receive_message3_metadata(key) {
            Err(FourWayError::CryptoNotVerified) => {}
            Err(error) => { self.fail(); return Err(SupplicantError::Handshake(error)); }
            Ok(()) => { self.fail(); return Err(SupplicantError::Handshake(FourWayError::CryptoNotVerified)); }
        }
        let Some(ptk) = self.ptk.as_ref() else { self.fail(); return Err(SupplicantError::WrongState); };
        if key.nonce != self.handshake.anonce() { self.fail(); return Err(SupplicantError::WrongNonce); }
        if let Err(error)=wifi_crypto::verify_eapol_mic(ptk.kck(),message3){self.fail();return Err(SupplicantError::Crypto(error));}
        if !key.key_info.encrypted_key_data() || key.key_data.is_empty(){self.fail();return Err(SupplicantError::Gtk(GtkError::MissingGtk));}
        let gtk=match wifi_gtk::unwrap_gtk(ptk.kek(),key.key_data){Ok(v)=>v,Err(e)=>{self.fail();return Err(SupplicantError::Gtk(e));}};
        match self.group.install_verified(key.replay_counter,gtk){
            Ok(InstallOutcome::Installed)=>{}
            Ok(InstallOutcome::Retransmission)|Err(_)=>{self.fail();return Err(SupplicantError::WrongState);}
        }
        if let Err(e)=self.handshake.mark_message3_verified(key.replay_counter){self.fail();return Err(SupplicantError::Handshake(e));}
        let len=build_message4(ptk,key.replay_counter,output)?;
        self.completed_replay=Some(key.replay_counter);
        self.state=SupplicantState::Completed;
        Ok(len)
    }
    pub fn temporal_key(&self) -> Option<&[u8]> {
        self.ptk.as_ref().map(Ptk::tk)
    }

    pub fn gtk_install_count(&self) -> u64 {
        self.group.install_count()
    }

    pub fn gtk_installed(&self) -> bool {
        self.group.gtk().is_some()
    }

    pub fn fail(&mut self) {
        self.handshake.fail();
        self.ptk = None;
        self.group.clear();
        self.completed_replay = None;
        self.snonce.zeroize();
        self.authenticator = [0; 6];
        self.state = SupplicantState::Failed;
    }
}

impl Drop for Wpa2Supplicant {
    fn drop(&mut self) {
        self.ptk = None;
        self.snonce.zeroize();
        self.authenticator.zeroize();
    }
}

fn build_eapol_key(
    output: &mut [u8],
    key_info: u16,
    replay_counter: u64,
    nonce: [u8; 32],
) -> Result<usize, SupplicantError> {
    if output.len() < EAPOL_KEY_FRAME_LEN {
        return Err(SupplicantError::BufferTooSmall);
    }
    output[..EAPOL_KEY_FRAME_LEN].fill(0);
    output[0] = 2;
    output[1] = wifi_rsn::EAPOL_TYPE_KEY;
    output[2..4].copy_from_slice(&(KEY_BODY_LEN as u16).to_be_bytes());
    output[4] = wifi_rsn::EAPOL_KEY_DESCRIPTOR_RSN;
    output[5..7].copy_from_slice(&key_info.to_be_bytes());
    output[9..17].copy_from_slice(&replay_counter.to_be_bytes());
    output[17..49].copy_from_slice(&nonce);
    output[97..99].copy_from_slice(&0u16.to_be_bytes());
    Ok(EAPOL_KEY_FRAME_LEN)
}

fn build_message4(ptk:&Ptk,replay:u64,out:&mut[u8])->Result<usize,SupplicantError>{
    let len=build_eapol_key(out,KEY_INFO_PAIRWISE|KEY_INFO_MIC|KEY_INFO_SECURE|KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES,replay,[0;32])?;
    let mic=wifi_crypto::compute_eapol_mic(ptk.kck(),&out[..len])?;
    out[81..97].copy_from_slice(&mic);Ok(len)
}
fn build_eapol_key_with_data(out:&mut[u8],info:u16,replay:u64,nonce:[u8;32],data:&[u8])->Result<usize,SupplicantError>{
    let body=KEY_BODY_LEN.checked_add(data.len()).ok_or(SupplicantError::BufferTooSmall)?;
    let total=EAPOL_HEADER_LEN.checked_add(body).ok_or(SupplicantError::BufferTooSmall)?;
    if out.len()<total||body>u16::MAX as usize||data.len()>u16::MAX as usize{return Err(SupplicantError::BufferTooSmall)}
    out[..total].fill(0);out[0]=2;out[1]=wifi_rsn::EAPOL_TYPE_KEY;out[2..4].copy_from_slice(&(body as u16).to_be_bytes());
    out[4]=wifi_rsn::EAPOL_KEY_DESCRIPTOR_RSN;out[5..7].copy_from_slice(&info.to_be_bytes());out[9..17].copy_from_slice(&replay.to_be_bytes());
    out[17..49].copy_from_slice(&nonce);out[97..99].copy_from_slice(&(data.len() as u16).to_be_bytes());out[99..total].copy_from_slice(data);Ok(total)
}
fn sign_frame(ptk: &Ptk, frame: &mut [u8]) -> Result<(), SupplicantError> {
    let mic = wifi_crypto::compute_eapol_mic(ptk.kck(), frame)?;
    frame[81..97].copy_from_slice(&mic);
    Ok(())
}

pub fn self_test() -> bool {
    let rsn=[1,0,0,0x0f,0xac,4,1,0,0,0x0f,0xac,4,1,0,0,0x0f,0xac,2,0,0];
    let Ok(profile)=wifi_rsn::parse_rsn(&rsn)else{return false};
    let Ok(pmk)=wifi_crypto::derive_pmk(b"password",b"IEEE")else{return false};
    let station=[0x00,0x13,0x46,0xfe,0x32,0x0c];let ap=[0x00,0x14,0x6c,0x7e,0x40,0x80];
    let snonce=[0x22;32];let anonce=[0x11;32];let mut supplicant=Wpa2Supplicant::new(station);
    if supplicant.begin(profile,ap,snonce).is_err(){return false}
    let mut msg1=[0u8;EAPOL_KEY_FRAME_LEN];
    let Ok(n1)=build_eapol_key(&mut msg1,KEY_INFO_PAIRWISE|(1<<7)|KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES,1,anonce)else{return false};
    let mut msg2=[0u8;EAPOL_KEY_FRAME_LEN];
    if supplicant.receive_message1_build_message2(&pmk,&msg1[..n1],&mut msg2).is_err(){return false}
    let Ok(ap_ptk)=wifi_crypto::derive_ptk(&pmk,ap,station,anonce,snonce)else{return false};
    let mut wrapped=[0u8;40];let Ok(wn)=wifi_gtk::wrap_gtk_for_test(ap_ptk.kek(),1,[0x5a;wifi_gtk::GTK_LEN],&mut wrapped)else{return false};
    let mut msg3=[0u8;160];let info=KEY_INFO_PAIRWISE|(1<<6)|(1<<7)|KEY_INFO_MIC|KEY_INFO_SECURE|(1<<12)|KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES;
    let Ok(n3)=build_eapol_key_with_data(&mut msg3,info,2,anonce,&wrapped[..wn])else{return false};
    if sign_frame(&ap_ptk,&mut msg3[..n3]).is_err(){return false}
    let mut msg4=[0u8;EAPOL_KEY_FRAME_LEN];
    let Ok(n4)=supplicant.receive_message3_build_message4(&msg3[..n3],&mut msg4)else{return false};
    if supplicant.state()!=SupplicantState::Completed||supplicant.handshake.state()!=FourWayState::Completed||!supplicant.gtk_installed()||supplicant.gtk_install_count()!=1||wifi_crypto::verify_eapol_mic(ap_ptk.kck(),&msg4[..n4]).is_err(){return false}
    let mut retry=[0u8;EAPOL_KEY_FRAME_LEN];
    if supplicant.receive_message3_build_message4(&msg3[..n3],&mut retry).is_err()||supplicant.gtk_install_count()!=1{return false}
    msg3[99]^=1;
    if supplicant.receive_message3_build_message4(&msg3[..n3],&mut retry).is_ok()||supplicant.gtk_install_count()!=1{return false}
    true
}