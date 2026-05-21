pub fn take_next_aud_access_unit(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let first_start = find_next_start_code(buffer, 0)?;
    if first_start > 0 {
        buffer.drain(..first_start);
    }

    let first_aud = find_next_aud(buffer, 0)?;
    if first_aud > 0 {
        buffer.drain(..first_aud);
    }

    let second_aud = find_next_aud(buffer, start_code_len(buffer)? + 1)?;
    Some(buffer.drain(..second_aud).collect())
}

fn find_next_aud(input: &[u8], start: usize) -> Option<usize> {
    let mut offset = start;
    while let Some(start_code) = find_next_start_code(input, offset) {
        let nal_start = start_code + start_code_len(&input[start_code..])?;
        if nal_start < input.len() && input[nal_start] & 0x1f == 9 {
            return Some(start_code);
        }
        offset = nal_start.saturating_add(1);
    }
    None
}

fn start_code_len(input: &[u8]) -> Option<usize> {
    if input.starts_with(&[0, 0, 0, 1]) {
        Some(4)
    } else if input.starts_with(&[0, 0, 1]) {
        Some(3)
    } else {
        None
    }
}

fn find_next_start_code(input: &[u8], start: usize) -> Option<usize> {
    let mut offset = start;
    while offset + 3 <= input.len() {
        if input[offset..].starts_with(&[0, 0, 0, 1]) || input[offset..].starts_with(&[0, 0, 1]) {
            return Some(offset);
        }
        offset += 1;
    }
    None
}
