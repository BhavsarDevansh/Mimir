pub fn start_placeholder() {
    println!("mimir-server placeholder started");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_placeholder() {
        start_placeholder();
    }
}
