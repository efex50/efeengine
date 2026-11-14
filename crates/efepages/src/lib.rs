pub mod page;
pub mod vars;










#[cfg(test)]
mod tests{
    use std::usize;

    use crate::{page::Page, vars::PAGE_SIZE};
    #[test]
    fn paget1(){
        let mut p = Page::default();
        p.page.insert(10, vec![12,32,32,3]);
        p.page.insert(15, vec![12,32,32,3]);
        p.page.insert(usize::MAX, vec![12,32,32,3]);
        println!("{:?}",p)
    }
    #[test]
    fn pt2(){
        println!("{}",1025 % PAGE_SIZE)
    }
    #[test]
    fn pt3(){
        let mut page = Page::default();
        page.write(24, vec![20;10000]);
        println!("{:?}",page);
        println!("{:?}",page.page.get(&0).unwrap().len());
    }
    #[test]
    fn pt4(){
        let a = vec![20;200];
        let b = a[0..20].to_vec();
        println!("{:?}",b[19]);
        println!("len {}",b.len())
        
    }

    #[test]
    fn pt5(){
        let mut p = Page::default();
        p.write(2481152 - 100, vec![10;100]);
        println!("{:?}",p);

    }

    #[test]
    fn pt6(){
        let p = Page::default();
        let r = p.read(1000, 24);
        println!("{:?}",r);
    }
    #[test]
    fn pt7(){
        let mut p = Page::default(); 
        p.write(250, vec![31;500]);
        let r = p.read(730, 24);
        println!("{:?}",r);
    }

    #[test]
    fn pt8(){
        let mut p = Page::default();
        let len = 2400;
        p.write(250, vec![31;50000]);
        let r = p.read(1000, len);
        println!("{:?}",r);
        assert_eq!(r.len() , len);

    }

    #[test]
    fn pt9(){
        let mut p = Page::default();
        p.write(0, vec![10;10]);

        p.write(5, vec![11;10]);
        println!("{:?}",p)
    }

    #[test]
    fn malloc(){
        let mut p = Page::default();
        p.write(0, vec![10;100]);
        dbg!("OK!");
        p.write(18446744073709550592 - 200, vec![255;200]);
        println!("{:?}",p);

    }

    #[test]
    fn test_a_multi_page_boundary_write_read() {
        let mut p = Page::default();
        let pattern: Vec<u8> = (0..8192).map(|i| (i % 251) as u8).collect();
        p.write(0, pattern.clone());
        let read_back = p.read(0, 8192);
        assert_eq!(read_back, pattern);
    }

    #[test]
    fn test_b_mid_page_to_boundary_across_three_pages() {
        let mut p = Page::default();
        let start = 1000;
        let end = 3 * PAGE_SIZE;
        let len = end - start;
        let pattern: Vec<u8> = (0..len).map(|i| ((i * 7 + 11) % 256) as u8).collect();
        p.write(start, pattern.clone());
        let read_back = p.read(start, len);
        assert_eq!(read_back, pattern);
    }

    #[test]
    fn test_c_boundary_within_one_page() {
        let mut p = Page::default();
        let start = 1500;
        let end = PAGE_SIZE;
        let len = end - start;
        let pattern: Vec<u8> = (0..len).map(|i| ((i * 13 + 5) % 256) as u8).collect();
        p.write(start, pattern.clone());
        let read_back = p.read(start, len);
        assert_eq!(read_back, pattern);

        let start_full = PAGE_SIZE;
        let len_full = PAGE_SIZE;
        let pattern_full: Vec<u8> = (0..len_full).map(|i| ((i * 17 + 1) % 256) as u8).collect();
        p.write(start_full, pattern_full.clone());
        let read_full = p.read(start_full, len_full);
        assert_eq!(read_full, pattern_full);
    }

    #[test]
    fn test_d_zero_length_reads_and_writes() {
        let mut p = Page::default();
        assert_eq!(p.read(4096, 0), Vec::<u8>::new());
        assert_eq!(p.read(0, 0), Vec::<u8>::new());
        assert_eq!(p.read(PAGE_SIZE * 2, 0), Vec::<u8>::new());
        assert_eq!(p.read(1234, 0), Vec::<u8>::new());

        p.write(4096, Vec::new());
        p.write(0, Vec::new());
        p.write(1234, Vec::new());
        assert!(p.page.is_empty());
    }

    #[test]
    fn test_e_unwritten_memory_returns_zeros() {
        let p = Page::default();
        let len = 5000;
        let r = p.read(2000, len);
        assert_eq!(r.len(), len);
        assert_eq!(r, vec![0u8; len]);

        let r2 = p.read(0, PAGE_SIZE * 2);
        assert_eq!(r2.len(), PAGE_SIZE * 2);
        assert_eq!(r2, vec![0u8; PAGE_SIZE * 2]);
    }

    #[test]
    fn test_f_spanning_boundary_by_few_bytes() {
        let mut p = Page::default();
        let start = 4090;
        let len = 12;
        let pattern: Vec<u8> = (0..len).map(|i| (i + 1) as u8).collect();
        p.write(start, pattern.clone());
        let r = p.read(start, len);
        assert_eq!(r, pattern);

        let mut p2 = Page::default();
        let pattern2: Vec<u8> = (0..4106).map(|i| (i % 255 + 1) as u8).collect();
        p2.write(4086, pattern2.clone());
        let r2 = p2.read(4086, 4106);
        assert_eq!(r2, pattern2);
    }

    #[test]
    fn test_g_randomised_loop_against_reference_model() {
        let total_size = 5 * PAGE_SIZE;
        let mut reference = vec![0u8; total_size];
        let mut p = Page::default();

        let mut rng_state: u64 = 0x853c49e6748fea9b;
        let mut next_rand = || {
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (rng_state >> 32) as usize
        };

        for step in 0..100 {
            let start = next_rand() % total_size;
            let max_len = total_size - start;
            let len = match step % 5 {
                0 => 0,
                1 => max_len,
                2 => {
                    let next_boundary = ((start / PAGE_SIZE) + 1) * PAGE_SIZE;
                    if next_boundary <= total_size {
                        next_boundary - start
                    } else {
                        max_len
                    }
                }
                _ => next_rand() % (max_len + 1),
            };

            let pattern: Vec<u8> = (0..len).map(|i| ((step * 13 + i) % 255 + 1) as u8).collect();
            p.write(start, pattern.clone());
            reference[start..start + len].copy_from_slice(&pattern);

            let read_back = p.read(start, len);
            assert_eq!(read_back, pattern);

            let r_start = next_rand() % total_size;
            let r_max = total_size - r_start;
            let r_len = next_rand() % (r_max + 1);
            let r_page = p.read(r_start, r_len);
            let r_ref = &reference[r_start..r_start + r_len];
            assert_eq!(r_page.as_slice(), r_ref);
        }

        let full_page = p.read(0, total_size);
        assert_eq!(full_page, reference);
    }

    #[test]
    fn test_overflow_no_panic() {
        let mut p = Page::default();
        let _ = p.read(usize::MAX - 10, 100);
        let _ = p.read(usize::MAX, 1);
        p.write(usize::MAX - 10, vec![42; 100]);
        p.write(usize::MAX, vec![42; 1]);
    }
}