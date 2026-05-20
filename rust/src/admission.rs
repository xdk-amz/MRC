pub trait AdmissionPolicy: Send {
    /// On first touch, returns true if key should be admitted to DRAM, false for flash.
    fn admit_to_dram(&mut self, key: u64, t: u64) -> bool;
    fn label(&self) -> &'static str;
}

/// Always admit new keys to DRAM (current default behavior).
pub struct AdmitToDram;

impl AdmissionPolicy for AdmitToDram {
    #[inline]
    fn admit_to_dram(&mut self, _key: u64, _t: u64) -> bool { true }
    fn label(&self) -> &'static str { "admit-dram" }
}

/// Admit new keys directly to flash (DRAM only via promotion).
pub struct AdmitToFlash;

impl AdmissionPolicy for AdmitToFlash {
    #[inline]
    fn admit_to_dram(&mut self, _key: u64, _t: u64) -> bool { false }
    fn label(&self) -> &'static str { "admit-flash" }
}
