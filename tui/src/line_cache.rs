use crate::render::text_utils::StyledLine;

#[derive(Clone, Debug)]
pub(crate) struct LinesCache<K> {
    key: Option<K>,
    pub(crate) lines: Vec<StyledLine>,
    #[cfg(test)]
    pub(crate) hits: u64,
    #[cfg(test)]
    pub(crate) misses: u64,
}

impl<K> Default for LinesCache<K> {
    fn default() -> Self {
        Self {
            key: None,
            lines: Vec::new(),
            #[cfg(test)]
            hits: 0,
            #[cfg(test)]
            misses: 0,
        }
    }
}

impl<K: PartialEq> LinesCache<K> {
    pub(crate) fn invalidate(&mut self) {
        self.key = None;
    }

    pub(crate) fn refresh(&mut self, key: K, build_lines: impl FnOnce() -> Vec<StyledLine>) {
        if self.key.as_ref() == Some(&key) {
            #[cfg(test)]
            {
                self.hits += 1;
            }
            return;
        }

        self.key = Some(key);
        self.lines = build_lines();
        #[cfg(test)]
        {
            self.misses += 1;
        }
    }
}
