use rand::RngCore;
use sha2::{Digest, Sha256};

pub const SEED_BYTES: usize = 32;

pub fn new_seed() -> Vec<u8> {
    let mut seed = vec![0u8; SEED_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    seed
}

pub fn seed_text(seed: &[u8]) -> String {
    hex::encode(seed)
}

fn sha256(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

pub fn seed_hash(seed: &[u8]) -> String {
    sha256(&seed_text(seed))
}

pub fn ticket(seed: &[u8], client_id: i64) -> String {
    sha256(&format!("{}:{client_id}", seed_text(seed)))
}

pub fn list_hash(ids: &[i64]) -> String {
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let text: Vec<String> = sorted.iter().map(i64::to_string).collect();
    sha256(&text.join("\n"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drawn {
    pub client_id: i64,
    pub ticket: String,
}

pub fn tickets(seed: &[u8], ids: &[i64]) -> Vec<Drawn> {
    let mut drawn: Vec<Drawn> = ids
        .iter()
        .map(|id| Drawn {
            client_id: *id,
            ticket: ticket(seed, *id),
        })
        .collect();
    drawn.sort_by(|a, b| a.ticket.cmp(&b.ticket).then(a.client_id.cmp(&b.client_id)));
    drawn.dedup_by_key(|d| d.client_id);
    drawn
}

pub fn short(hash: &str) -> String {
    if hash.len() <= 12 {
        return hash.to_string();
    }
    format!("{}…{}", &hash[..8], &hash[hash.len() - 4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn жеребьёвка_повторяется_по_зерну_и_не_зависит_от_порядка() {
        let seed = vec![7u8; SEED_BYTES];
        let a = tickets(&seed, &[5, 1, 9, 3]);
        let b = tickets(&seed, &[9, 3, 5, 1]);
        assert_eq!(a, b);
        assert_eq!(a.len(), 4);
        assert!(a.windows(2).all(|w| w[0].ticket <= w[1].ticket));
        let other = tickets(&[8u8; SEED_BYTES], &[5, 1, 9, 3]);
        assert_ne!(
            a.iter().map(|d| d.client_id).collect::<Vec<_>>(),
            other.iter().map(|d| d.client_id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn отпечатки_считаются_как_в_подсказке_для_проверки() {
        let seed = vec![0u8; SEED_BYTES];
        assert_eq!(seed_text(&seed), "0".repeat(64));
        assert_eq!(
            seed_hash(&seed),
            hex::encode(Sha256::digest("0".repeat(64).as_bytes()))
        );
        assert_eq!(
            ticket(&seed, 42),
            hex::encode(Sha256::digest(format!("{}:42", "0".repeat(64)).as_bytes()))
        );
        assert_eq!(list_hash(&[3, 1, 2, 2]), list_hash(&[1, 2, 3]));
        assert_eq!(
            list_hash(&[1, 2, 3]),
            hex::encode(Sha256::digest("1\n2\n3".as_bytes()))
        );
        assert_eq!(new_seed().len(), SEED_BYTES);
        assert_ne!(new_seed(), new_seed());
        assert_eq!(short("abcdef0123456789"), "abcdef01…6789");
    }
}
