//! Maps session ids to live items. Attaching an id that already exists returns the existing item, so a
//! second attach (for example a webview reload) never spawns a second PTY.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

pub struct Registry<T> {
    items: Mutex<HashMap<String, Arc<T>>>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Registry { items: Mutex::new(HashMap::new()) }
    }
}

impl<T> Registry<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the item for `id`, and whether this call created it. `make` runs only for a new id, under the
    /// lock, so two concurrent attaches cannot both create one.
    pub fn get_or_insert_with<E>(&self, id: &str, make: impl FnOnce() -> Result<T, E>) -> Result<(Arc<T>, bool), E> {
        let mut items = self.lock();
        if let Some(item) = items.get(id) {
            return Ok((item.clone(), false));
        }
        let item = Arc::new(make()?);
        items.insert(id.to_string(), item.clone());
        Ok((item, true))
    }

    pub fn get(&self, id: &str) -> Option<Arc<T>> {
        self.lock().get(id).cloned()
    }

    pub fn remove(&self, id: &str) -> Option<Arc<T>> {
        self.lock().remove(id)
    }

    /// Removes every item (app shutdown).
    pub fn take_all(&self) -> Vec<Arc<T>> {
        self.lock().drain().map(|(_, item)| item).collect()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<T>>> {
        self.items.lock().unwrap_or_else(|e| e.into_inner())
    }
}
