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

    /// Removes the item for `id` only when `pred` holds for it. The check and the removal happen under one lock.
    pub fn remove_if(&self, id: &str, pred: impl FnOnce(&T) -> bool) -> Option<Arc<T>> {
        let mut items = self.lock();
        let hit = items.get(id).is_some_and(|item| pred(item));
        if hit { items.remove(id) } else { None }
    }

    /// Removes every item (app shutdown).
    pub fn take_all(&self) -> Vec<Arc<T>> {
        self.lock().drain().map(|(_, item)| item).collect()
    }

    /// Every item, still registered. The order is unspecified.
    pub fn snapshot(&self) -> Vec<Arc<T>> {
        self.lock().values().cloned().collect()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<T>>> {
        self.items.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_if_removes_only_when_the_predicate_holds() {
        let reg: Registry<u32> = Registry::new();
        reg.get_or_insert_with::<()>("a", || Ok(1)).expect("insert");
        assert!(reg.remove_if("a", |v| *v == 2).is_none());
        assert!(reg.get("a").is_some(), "a refused removal must keep the item");
        assert_eq!(reg.remove_if("a", |v| *v == 1).map(|v| *v), Some(1));
        assert!(reg.get("a").is_none());
        assert!(reg.remove_if("a", |_| true).is_none(), "a missing id removes nothing");
    }

    #[test]
    fn snapshot_lists_every_item_and_keeps_them_registered() {
        let reg: Registry<u32> = Registry::new();
        reg.get_or_insert_with::<()>("a", || Ok(1)).expect("insert");
        reg.get_or_insert_with::<()>("b", || Ok(2)).expect("insert");
        let mut values: Vec<u32> = reg.snapshot().iter().map(|v| **v).collect();
        values.sort();
        assert_eq!(values, vec![1, 2]);
        assert!(reg.get("a").is_some() && reg.get("b").is_some(), "a snapshot removes nothing");
    }
}
