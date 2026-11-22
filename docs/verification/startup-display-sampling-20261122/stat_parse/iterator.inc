        let mut fields = tail.split_ascii_whitespace();
        let state_field = fields.next();
        let parent_field = fields.next();
        let group_field = fields.next();
        let session_field = fields.next();
        let birth_field = fields.nth(15);
        let (Some(state), Some(parent_text), Some(group_text), Some(session_text), Some(birth_text)) =
            (state_field, parent_field, group_field, session_field, birth_field) else {
                anyhow::bail!("The native-read process is not alive");
            };
        ensure!(!matches!(state, "Z" | "X" | "x"), "The native-read process is not alive");
        let parent = parse_pid(parent_text)?;
        let group = parse_pid(group_text)?;
        let session = parse_pid(session_text)?;
        let birth = birth_text.parse::<u64>()?;
