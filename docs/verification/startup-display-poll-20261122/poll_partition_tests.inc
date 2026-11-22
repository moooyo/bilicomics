#[cfg(test)]
mod poll_partition_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn poll_children_use_exactly_the_parent_clock_pair_and_cover_every_call() {
        let stamps = [100, 130, 140, 170, 200, 200];
        let index = Cell::new(0);
        let clock = || {
            let stamp = stamps[index.get()];
            index.set(index.get() + 1);
            Some(stamp)
        };
        ACTIVE.with(|active| *active.borrow_mut() = Some(Totals::default()));
        for (fd, expected) in [(13, 1), (-1, 0), (0, 1)] {
            assert_eq!(observe_poll_with(fd, || expected, clock), expected);
        }
        let totals = ACTIVE.with(|active| active.borrow_mut().take().unwrap());
        assert_eq!(index.get(), 6);
        assert!(!totals.invalid_clock);
        let parent = totals.buckets[Bucket::CommandPoll as usize];
        let open = totals.buckets[Bucket::CommandPollStdoutOpen as usize];
        let eof = totals.buckets[Bucket::CommandPollAfterEof as usize];
        assert_eq!((open.count, open.elapsed_ns), (2, 30));
        assert_eq!((eof.count, eof.elapsed_ns), (1, 30));
        assert_eq!((parent.count, parent.elapsed_ns), (open.count + eof.count, open.elapsed_ns + eof.elapsed_ns));
    }

    #[test]
    fn poll_observation_preserves_failure_errno_and_disabled_path_does_not_sample() {
        assert_eq!(observe_poll_with(15, || -1, || panic!("disabled poll read a clock")), -1);
        ACTIVE.with(|active| *active.borrow_mut() = Some(Totals::default()));
        let result = observe_poll(-1, || {
            unsafe { *libc::__errno_location() = libc::EINTR };
            -1
        });
        let errno = io::Error::last_os_error().raw_os_error();
        let totals = ACTIVE.with(|active| active.borrow_mut().take().unwrap());
        assert_eq!(result, -1);
        assert_eq!(errno, Some(libc::EINTR));
        assert_eq!(totals.buckets[Bucket::CommandPoll as usize].count, 1);
        assert_eq!(totals.buckets[Bucket::CommandPollAfterEof as usize].count, 1);
        assert_eq!(totals.buckets[Bucket::CommandPollStdoutOpen as usize].count, 0);
    }
}
