//! A small bounded FIFO shared by the sound-cue and viewmodel-action output
//! queues, mirroring `ohl_combat::CombatEventQueue`'s "drop the overflow,
//! count it" policy: a busy tick can never make this crate's presentation
//! output grow without bound, and the caller can tell that it happened.

/// A bounded, order-preserving queue. Pushing past `capacity` drops the new
/// item and counts it rather than growing or evicting an older entry, so a
/// drained queue's contents are always exactly what was pushed, in order,
/// up to the point it started overflowing.
#[derive(Debug, Clone)]
pub(crate) struct BoundedQueue<T> {
    items: Vec<T>,
    capacity: usize,
    dropped: usize,
}

impl<T> BoundedQueue<T> {
    /// An empty queue holding at most `capacity` items (at least one).
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            items: Vec::with_capacity(capacity),
            capacity,
            dropped: 0,
        }
    }

    /// Appends `item`, returning `false` when the queue was full and `item`
    /// was dropped instead.
    pub(crate) fn push(&mut self, item: T) -> bool {
        if self.items.len() >= self.capacity {
            self.dropped += 1;
            return false;
        }
        self.items.push(item);
        true
    }

    /// The queued items, oldest first.
    pub(crate) fn items(&self) -> &[T] {
        &self.items
    }

    /// How many items have been dropped since the last drain/clear.
    pub(crate) fn dropped(&self) -> usize {
        self.dropped
    }

    /// The queue's capacity.
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Drains every queued item, oldest first, resetting the dropped
    /// counter.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        self.dropped = 0;
        self.items.drain(..)
    }
}

#[cfg(test)]
mod tests {
    use super::BoundedQueue;

    #[test]
    fn pushing_past_capacity_drops_and_counts_instead_of_growing() {
        let mut queue: BoundedQueue<u32> = BoundedQueue::with_capacity(2);
        assert!(queue.push(1));
        assert!(queue.push(2));
        assert!(!queue.push(3));
        assert_eq!(queue.items(), [1, 2]);
        assert_eq!(queue.dropped(), 1);
        assert_eq!(queue.capacity(), 2);
    }

    #[test]
    fn draining_yields_items_in_order_and_resets_the_drop_count() {
        let mut queue: BoundedQueue<u32> = BoundedQueue::with_capacity(4);
        queue.push(1);
        queue.push(2);
        queue.push(3);
        queue.push(4);
        queue.push(5);
        assert_eq!(queue.dropped(), 1);
        let drained: Vec<u32> = queue.drain().collect();
        assert_eq!(drained, [1, 2, 3, 4]);
        assert_eq!(queue.dropped(), 0);
        assert!(queue.items().is_empty());
    }

    #[test]
    fn a_zero_capacity_request_is_raised_to_one() {
        let queue: BoundedQueue<u32> = BoundedQueue::with_capacity(0);
        assert_eq!(queue.capacity(), 1);
    }
}
