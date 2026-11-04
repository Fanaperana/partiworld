use std::collections::HashMap;

pub trait Def {
    fn def_id(&self) -> &str;
}

/// Ordered store of definitions keyed by string id.
/// Inserting an id that already exists replaces it in place (mod override),
/// so numeric indices stay stable.
#[derive(Clone, Debug)]
pub struct Registry<T> {
    items: Vec<T>,
    sources: Vec<String>,
    index: HashMap<String, usize>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self { items: Vec::new(), sources: Vec::new(), index: HashMap::new() }
    }
}

impl<T: Def> Registry<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace. Returns the index.
    pub fn insert(&mut self, def: T, source: &str) -> usize {
        let id = def.def_id().to_string();
        if let Some(&i) = self.index.get(&id) {
            self.items[i] = def;
            self.sources[i] = source.to_string();
            i
        } else {
            let i = self.items.len();
            self.items.push(def);
            self.sources.push(source.to_string());
            self.index.insert(id, i);
            i
        }
    }

    pub fn idx(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }
    pub fn get(&self, id: &str) -> Option<&T> {
        self.idx(id).map(|i| &self.items[i])
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut T> {
        self.idx(id).map(|i| &mut self.items[i])
    }
    pub fn by_idx(&self, i: usize) -> &T {
        &self.items[i]
    }
    pub fn try_idx(&self, i: usize) -> Option<&T> {
        self.items.get(i)
    }
    /// Id of the mod that last defined this entry.
    pub fn source(&self, i: usize) -> &str {
        &self.sources[i]
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (usize, &T)> {
        self.items.iter().enumerate()
    }
    pub fn ids(&self) -> Vec<String> {
        self.items.iter().map(|d| d.def_id().to_string()).collect()
    }
}
