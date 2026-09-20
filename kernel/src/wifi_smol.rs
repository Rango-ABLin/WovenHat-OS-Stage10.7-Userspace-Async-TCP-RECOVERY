//! WovenWiFi Stage 13.9U — smoltcp Ethernet device adapter.
//!
//! This adapter deliberately sits above `WifiSession`: smoltcp only sees
//! Ethernet frames, while association/WPA2/CCMP/802.11 remain below it.
//! The existing virtio-net device remains the default production/recovery path.

use smoltcp::{
    phy::{ChecksumCapabilities,Device,DeviceCapabilities,Medium,RxToken,TxToken},
    time::Instant,
};

use crate::wifi_session::{SessionError,WifiSession};

pub const ETHERNET_MTU:usize=1514;

pub struct WifiSmolDevice<'a>{
    session:&'a mut WifiSession,
    epoch:u32,
    rx:[u8;ETHERNET_MTU],
}
impl<'a> WifiSmolDevice<'a>{
    pub fn new(session:&'a mut WifiSession,epoch:u32)->Self{
        Self{session,epoch,rx:[0;ETHERNET_MTU]}
    }
}

pub struct WifiRxToken<'a>{data:&'a mut[u8]}
pub struct WifiTxToken<'a>{session:&'a mut WifiSession,epoch:u32}

impl RxToken for WifiRxToken<'_>{
    fn consume<R,F>(self,f:F)->R where F:FnOnce(&[u8])->R{f(self.data)}
}
impl TxToken for WifiTxToken<'_>{
    fn consume<R,F>(self,len:usize,f:F)->R where F:FnOnce(&mut[u8])->R{
        let mut frame=[0u8;ETHERNET_MTU];
        let usable=core::cmp::min(len,ETHERNET_MTU);
        let result=f(&mut frame[..usable]);
        let _=self.session.transmit_ethernet(self.epoch,&frame[..usable]);
        result
    }
}

impl Device for WifiSmolDevice<'_>{
    type RxToken<'a>=WifiRxToken<'a> where Self:'a;
    type TxToken<'a>=WifiTxToken<'a> where Self:'a;

    fn receive(&mut self,_timestamp:Instant)->Option<(Self::RxToken<'_>,Self::TxToken<'_>)>{
        let len=match self.session.receive_ethernet(self.epoch,&mut self.rx){
            Ok(Some(n))=>n,
            Ok(None)|Err(_)=>return None,
        };
        Some((
            WifiRxToken{data:&mut self.rx[..len]},
            WifiTxToken{session:self.session,epoch:self.epoch},
        ))
    }

    fn transmit(&mut self,_timestamp:Instant)->Option<Self::TxToken<'_>>{
        self.session.is_active_epoch(self.epoch).then_some(
            WifiTxToken{session:self.session,epoch:self.epoch}
        )
    }

    fn capabilities(&self)->DeviceCapabilities{
        let mut caps=DeviceCapabilities::default();
        caps.medium=Medium::Ethernet;
        caps.max_transmission_unit=ETHERNET_MTU;
        caps.checksum=ChecksumCapabilities::default();
        caps
    }
}

#[cfg(feature="stage13-9-test")]
pub fn self_test()->bool{
    use smoltcp::phy::{Device,RxToken,TxToken};
    use crate::wifi_backend::MAX_80211_FRAME;

    let station=[0x02,0,0,0,0,1];
    let ap=[0x02,0,0,0,0,2];
    let peer=[0x02,0,0,0,0,3];
    let tk=[0x66;16];

    let mut session=WifiSession::new(2);
    let Ok(epoch)=session.begin_connect(1,10)else{return false};
    if session.install_pairwise(epoch,station,ap,&tk).is_err(){return false}

    // Exercise the exact smoltcp TxToken contract. The token receives an
    // Ethernet frame and must push a protected 802.11 frame into the backend.
    {
        let mut dev=WifiSmolDevice::new(&mut session,epoch);
        let Some(tx)=dev.transmit(Instant::from_millis(0))else{return false};
        tx.consume(64,|frame|{
            frame[..6].copy_from_slice(&peer);
            frame[6..12].copy_from_slice(&station);
            frame[12..14].copy_from_slice(&[0x08,0x00]);
            for(i,b)in frame[14..].iter_mut().enumerate(){*b=i as u8}
        });
    }
    let mut protected=[0u8;MAX_80211_FRAME];
    let Ok(Some(n))=session.dequeue_tx(&mut protected)else{return false};
    if n<=64{return false}

    // Disconnect must make the adapter stop advertising TX capability.
    let old=epoch;
    let new=session.disconnect();
    if new==old{return false}
    let mut dev=WifiSmolDevice::new(&mut session,old);
    if dev.transmit(Instant::from_millis(1)).is_some(){return false}

    true
}

pub fn classify_session_error(error:&SessionError)->bool{
    matches!(error,SessionError::StaleEpoch|SessionError::WrongState|SessionError::NoKeys)
}
