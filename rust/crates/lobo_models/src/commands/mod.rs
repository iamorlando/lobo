enum Command {
    Insert(OrderType),
}

#[cfg(test)]
struct OrderType;

#[cfg(test)]
mod tests {
    use super::{Command, OrderType};

    #[test]
    fn insert_command_can_be_constructed() {
        let command = Command::Insert(OrderType);
        assert!(matches!(command, Command::Insert(_)));
    }
}
