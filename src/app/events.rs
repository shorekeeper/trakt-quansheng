//! Eight foreground events with FIFO ordering.
//!
//! SysTick does not access this queue. Production and consumption both
//! happen in the foreground, so queue operations need no interrupt mask.
//! A full queue rejects the newest event and increments a saturating count.

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Service,
    Heartbeat,
}

#[derive(Clone, Copy)]
pub(crate) struct Event {
    pub(crate) kind: Kind,
    pub(crate) tick: u32,
}

pub(crate) struct Queue {
    slots: [Event; 8],
    head: usize,
    length: usize,
    pub(crate) dropped: u32,
}

impl Queue {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [Event { kind: Kind::Service, tick: 0 }; 8],
            head: 0,
            length: 0,
            dropped: 0,
        }
    }

    pub(crate) const fn push(&mut self, event: Event) -> bool {
        if self.length == 8 {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        let index = (self.head + self.length) & 7;
        self.slots[index] = event;
        self.length += 1;
        true
    }

    pub(crate) const fn pop(&mut self) -> Option<Event> {
        if self.length == 0 {
            return None;
        }
        let event = self.slots[self.head];
        self.head = (self.head + 1) & 7;
        self.length -= 1;
        Some(event)
    }
}

const _: () = {
    let mut queue = Queue::new();
    assert!(queue.pop().is_none());

    let mut index = 0u32;
    while index < 8 {
        assert!(queue.push(Event { kind: Kind::Heartbeat, tick: index }));
        index += 1;
    }
    assert!(!queue.push(Event { kind: Kind::Service, tick: 99 }));
    assert!(queue.dropped == 1);

    index = 0;
    while index < 8 {
        match queue.pop() {
            Some(event) => {
                assert!(event.tick == index);
                assert!(matches!(event.kind, Kind::Heartbeat));
            }
            None => panic!("queue lost an event"),
        }
        index += 1;
    }
    assert!(queue.pop().is_none());

    index = 0;
    while index < 32 {
        assert!(queue.push(Event { kind: Kind::Service, tick: index }));
        match queue.pop() {
            Some(event) => assert!(event.tick == index),
            None => panic!("queue wrap failed"),
        }
        index += 1;
    }

    queue.dropped = u32::MAX;
    index = 0;
    while index < 8 {
        assert!(queue.push(Event { kind: Kind::Service, tick: index }));
        index += 1;
    }
    assert!(!queue.push(Event { kind: Kind::Service, tick: 99 }));
    assert!(queue.dropped == u32::MAX);
};