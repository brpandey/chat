use std::collections::{HashMap};
use std::sync::Arc;
use std::net::SocketAddr;

use tokio::sync::{Mutex};
use tokio::net::tcp;

// current client registry data
pub type Registry = Arc<Mutex<HashMap<usize, RegistryEntry>>>;
pub type RegistryEntry = (SocketAddr, Vec<u8>, tcp::OwnedWriteHalf);

