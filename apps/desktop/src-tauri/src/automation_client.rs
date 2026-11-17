//! Keeps one UI Automation client alive on the thread that writes dictated
//! text, so live typing does not build a new one for every word.
//!
//! Only the client is reused: every question (which element is focused, is
//! it a password field) is asked again on each write. When a call reports
//! that the client's COM connection is gone, the client is rebuilt once and
//! the question asked again. The client type and the code that creates it
//! are passed in, so the rules are tested here without Windows. This file
//! has no dependencies so it also builds in `core-tests`.

/// COM error codes (HRESULTs) meaning the connection behind the client is
/// gone, so a new client may succeed where the old one failed. Time-outs are
/// not here: a hung app would only stall dictation a second time.
const CONNECTION_LOST: [u32; 6] = [
    0x8001_0108, // RPC_E_DISCONNECTED
    0x8001_0007, // RPC_E_SERVER_DIED
    0x8001_0012, // RPC_E_SERVER_DIED_DNE
    0x8004_01FD, // CO_E_OBJNOTCONNECTED
    0x8007_06BA, // RPC_S_SERVER_UNAVAILABLE
    0x8007_06BE, // RPC_S_CALL_FAILED
];

/// True when a failed call means the client itself should be rebuilt.
pub fn is_connection_lost(hresult: i32) -> bool {
    CONNECTION_LOST.contains(&(hresult as u32))
}

/// A client made on first use and kept for the life of its thread.
pub struct ReusedClient<C, F> {
    client: Option<C>,
    create: F,
}

impl<C, F: FnMut() -> Option<C>> ReusedClient<C, F> {
    /// `create` makes a new client, or `None` when that is not possible.
    pub fn new(create: F) -> Self {
        Self {
            client: None,
            create,
        }
    }

    /// Asks `ask` with the kept client, making it first if needed. A failure
    /// is the call's HRESULT. `None` when the client cannot be made or the
    /// question cannot be answered, so the caller decides what unknown means.
    pub fn query<R>(&mut self, mut ask: impl FnMut(&C) -> Result<R, i32>) -> Option<R> {
        for _ in 0..2 {
            if self.client.is_none() {
                self.client = (self.create)();
            }
            let client = self.client.as_ref()?;
            match ask(client) {
                Ok(answer) => return Some(answer),
                // Dropped here; the next attempt (or write) makes a new one.
                Err(code) if is_connection_lost(code) => self.client = None,
                Err(_) => return None,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const DISCONNECTED: i32 = 0x8001_0108_u32 as i32;
    const ELEMENT_NOT_AVAILABLE: i32 = 0x8004_0201_u32 as i32;
    const TIMEOUT: i32 = 0x8013_1505_u32 as i32;

    /// A stand-in client that remembers which creation it came from.
    struct FakeClient {
        id: u32,
    }

    #[test]
    fn makes_the_client_once_but_asks_on_every_write() {
        let created = Cell::new(0);
        let asked = Cell::new(0);
        let mut automation = ReusedClient::new(|| {
            created.set(created.get() + 1);
            Some(FakeClient { id: created.get() })
        });
        for write in 0..50 {
            // The answer changes between writes and must never be cached.
            let is_password = automation.query(|client| {
                asked.set(asked.get() + 1);
                assert_eq!(client.id, 1);
                Ok(write % 3 == 0)
            });
            assert_eq!(is_password, Some(write % 3 == 0));
        }
        assert_eq!(created.get(), 1);
        assert_eq!(asked.get(), 50);
    }

    #[test]
    fn a_lost_connection_rebuilds_the_client_once_and_asks_again() {
        let created = Cell::new(0);
        let mut automation = ReusedClient::new(|| {
            created.set(created.get() + 1);
            Some(FakeClient { id: created.get() })
        });
        assert_eq!(automation.query(|_| Ok(false)), Some(false));
        let answer = automation.query(|client| match client.id {
            1 => Err(DISCONNECTED),
            _ => Ok(true),
        });
        assert_eq!(answer, Some(true));
        assert_eq!(created.get(), 2);
        // The new client is kept for later writes.
        assert_eq!(automation.query(|client| Ok(client.id)), Some(2));
        assert_eq!(created.get(), 2);
    }

    #[test]
    fn a_connection_lost_twice_gives_up_and_the_next_write_starts_fresh() {
        let created = Cell::new(0);
        let asked = Cell::new(0);
        let mut automation = ReusedClient::new(|| {
            created.set(created.get() + 1);
            Some(FakeClient { id: created.get() })
        });
        let answer: Option<bool> = automation.query(|_| {
            asked.set(asked.get() + 1);
            Err(DISCONNECTED)
        });
        assert_eq!(answer, None);
        assert_eq!((created.get(), asked.get()), (2, 2));
        assert_eq!(automation.query(|client| Ok(client.id)), Some(3));
    }

    #[test]
    fn other_failures_keep_the_client_and_are_not_retried() {
        let created = Cell::new(0);
        let asked = Cell::new(0);
        let mut automation = ReusedClient::new(|| {
            created.set(created.get() + 1);
            Some(FakeClient { id: created.get() })
        });
        for code in [ELEMENT_NOT_AVAILABLE, TIMEOUT] {
            let answer: Option<bool> = automation.query(|_| {
                asked.set(asked.get() + 1);
                Err(code)
            });
            assert_eq!(answer, None);
        }
        assert_eq!((created.get(), asked.get()), (1, 2));
        assert_eq!(automation.query(|client| Ok(client.id)), Some(1));
    }

    #[test]
    fn a_client_that_cannot_be_made_is_tried_again_on_the_next_write() {
        let created = Cell::new(0);
        let mut automation = ReusedClient::new(|| {
            created.set(created.get() + 1);
            (created.get() > 1).then(|| FakeClient { id: created.get() })
        });
        assert_eq!(automation.query(|_| Ok(true)), None);
        assert_eq!(automation.query(|client| Ok(client.id)), Some(2));
        assert_eq!(created.get(), 2);
    }

    #[test]
    fn only_connection_errors_count_as_lost() {
        assert!(is_connection_lost(DISCONNECTED));
        assert!(is_connection_lost(0x8007_06BA_u32 as i32));
        assert!(!is_connection_lost(ELEMENT_NOT_AVAILABLE));
        assert!(!is_connection_lost(TIMEOUT));
        assert!(!is_connection_lost(0));
    }
}
