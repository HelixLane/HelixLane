//! Windows busy-instance retry policy, portable enough to exercise on every CI host.
use std::{io, time::Instant};
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) async fn open_until<T>(
    deadline: Instant,
    mut open: impl FnMut() -> io::Result<T>,
) -> io::Result<Option<T>> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Launch deadline"));
        }
        match open() {
            Ok(v) => return Ok(Some(v)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) if e.raw_os_error() == Some(231) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "Launch deadline"));
                }
                tokio::time::sleep(remaining.min(std::time::Duration::from_millis(10))).await;
            }
            Err(e) => return Err(e),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn only_busy_retries() {
        let until = Instant::now() + std::time::Duration::from_secs(1);
        let mut n = 0;
        let result = open_until(until, || {
            n += 1;
            if n < 3 {
                Err(io::Error::from_raw_os_error(231))
            } else {
                Ok(42)
            }
        })
        .await
        .unwrap();
        assert_eq!(result, Some(42));
        assert_eq!(n, 3);
        for code in [2, 5, 9999] {
            let mut n = 0;
            let result = open_until::<()>(until, || {
                n += 1;
                Err(io::Error::from_raw_os_error(code))
            })
            .await;
            assert_eq!(n, 1);
            if code == 2 {
                assert!(result.unwrap().is_none())
            } else {
                assert!(result.is_err())
            }
        }
    }
    #[tokio::test]
    async fn busy_expires() {
        let deadline = Instant::now() + std::time::Duration::from_millis(30);
        assert_eq!(
            open_until::<()>(deadline, || Err(io::Error::from_raw_os_error(231)))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
    #[tokio::test]
    async fn eight_concurrent_clients() {
        let mut handles = vec![];
        for _ in 0..8 {
            handles.push(tokio::spawn(async {
                let mut count = 0;
                open_until(Instant::now() + std::time::Duration::from_secs(1), || {
                    count += 1;
                    if count < 4 {
                        Err(io::Error::from_raw_os_error(231))
                    } else {
                        Ok(count)
                    }
                })
                .await
                .unwrap()
            }));
        }
        for h in handles {
            assert_eq!(h.await.unwrap(), Some(4));
        }
    }
}
