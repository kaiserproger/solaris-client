//! Pinned 26.1.2 MessageSignatureCache and LastSeenMessagesTracker transitions.
use std::sync::Arc;
pub type Signature = Arc<[u8]>;
pub struct SignatureCache {
    entries: [Option<Signature>; 128],
}
impl Default for SignatureCache {
    fn default() -> Self {
        Self {
            entries: std::array::from_fn(|_| None),
        }
    }
}
impl SignatureCache {
    pub fn get(&self, index: usize) -> Option<Signature> {
        self.entries.get(index)?.as_ref().map(Arc::clone)
    }
    pub fn push(&mut self, last_seen: &[Signature], signature: Option<&Signature>) {
        debug_assert!(last_seen.len() <= 20);
        let mut queue: [Option<Signature>; 21] = std::array::from_fn(|_| None);
        let mut len = last_seen.len();
        for (i, value) in last_seen.iter().enumerate() {
            queue[i] = Some(Arc::clone(value));
        }
        if let Some(value) = signature {
            queue[len] = Some(Arc::clone(value));
            len += 1;
        }
        let mut head = 0;
        for entry in &mut self.entries {
            if len == 0 {
                break;
            }
            len -= 1;
            let value = queue[(head + len) % 21].take();
            let previous = std::mem::replace(entry, value);
            if let Some(previous) = previous {
                if !last_seen.contains(&previous) && signature != Some(&previous) {
                    head = (head + 20) % 21;
                    queue[head] = Some(previous);
                    len += 1;
                }
            }
        }
    }
}
#[derive(Debug)]
pub struct LastSeenUpdate {
    pub offset: i32,
    pub acknowledged: [u8; 3],
    pub checksum: u8,
    pub signatures: Vec<Signature>,
}
pub struct LastSeenTracker {
    entries: [Option<(Signature, bool)>; 20],
    tail: usize,
    offset: i32,
    last_tracked: Option<Signature>,
}
impl Default for LastSeenTracker {
    fn default() -> Self {
        Self {
            entries: std::array::from_fn(|_| None),
            tail: 0,
            offset: 0,
            last_tracked: None,
        }
    }
}
impl LastSeenTracker {
    pub fn add_pending(&mut self, signature: Signature, accepted: bool) -> bool {
        if self.last_tracked.as_ref() == Some(&signature) {
            return false;
        }
        self.last_tracked = Some(Arc::clone(&signature));
        self.entries[self.tail] = accepted.then_some((signature, true));
        self.tail = (self.tail + 1) % 20;
        self.offset += 1;
        true
    }
    pub fn ignore_pending(&mut self, signature: &Signature) {
        for entry in &mut self.entries {
            if entry
                .as_ref()
                .is_some_and(|(value, pending)| *pending && value == signature)
            {
                *entry = None;
                break;
            }
        }
    }
    pub fn acknowledgement(&mut self) -> Option<i32> {
        (self.offset > 64).then(|| std::mem::take(&mut self.offset))
    }
    pub fn update(&mut self) -> LastSeenUpdate {
        let offset = std::mem::take(&mut self.offset);
        let mut acknowledged = [0; 3];
        let mut signatures = Vec::with_capacity(20);
        let mut checksum = 1i32;
        for i in 0..20 {
            if let Some((signature, pending)) = &mut self.entries[(self.tail + i) % 20] {
                acknowledged[i / 8] |= 1 << (i % 8);
                *pending = false;
                let hash = signature.iter().fold(1i32, |h, b| {
                    h.wrapping_mul(31).wrapping_add(*b as i8 as i32)
                });
                checksum = checksum.wrapping_mul(31).wrapping_add(hash);
                signatures.push(Arc::clone(signature));
            }
        }
        let checksum = checksum as u8;
        LastSeenUpdate {
            offset,
            acknowledged,
            checksum: if checksum == 0 { 1 } else { checksum },
            signatures,
        }
    }
}
