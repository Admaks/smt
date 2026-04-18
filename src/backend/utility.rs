pub fn u64_to_i32x2(value: u64)-> (i32, i32) {
    let high = (value >> 32) as i32;
    let low = (value & 0xFFFF_FFFF) as i32;
    (high, low)
}

pub fn i32c_to_u64(value : (i32, i32)) -> u64 {
    let (high, low) =  value;
    ((high as u64) << 32) | (low as u32 as u64)
}

pub fn i32x2_to_u64(id_1: i32, id_2: i32) -> u64 {
    i32c_to_u64((id_1, id_2))
}


#[test]
pub fn test() {
    println!("{:#?}", u64_to_i32x2(17607058970));
}
#[test]
pub fn r() {
    println!("{:#?}", i32x2_to_u64(4, 427189786));
}
