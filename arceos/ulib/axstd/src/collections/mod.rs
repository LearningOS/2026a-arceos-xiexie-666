//! Collection types.
//!
//! Everything from [`alloc::collections`] is re-exported here. In addition, a
//! [`HashMap`] is provided because `alloc` only ships the tree based maps.

#[doc(no_inline)]
pub use alloc::collections::*;

mod hashmap;

pub use hashmap::{HashMap, HashMapBuilder};
