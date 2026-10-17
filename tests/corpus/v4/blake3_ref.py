"""BLAKE3, written from the specification (https://github.com/BLAKE3-team/BLAKE3-specs).

A small, slow, single-threaded implementation of the unkeyed hash with a
32-byte output. It exists so `generate_v4.py` shares no code with velo. It
checks itself against a known vector when imported.
"""

IV = [0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
      0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19]
PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8]
CHUNK_START, CHUNK_END, PARENT, ROOT = 1, 2, 4, 8
M32 = 0xFFFFFFFF
CHUNK_LEN = 1024


def _rotr(x, n):
    return ((x >> n) | (x << (32 - n))) & M32


def _g(s, a, b, c, d, mx, my):
    s[a] = (s[a] + s[b] + mx) & M32
    s[d] = _rotr(s[d] ^ s[a], 16)
    s[c] = (s[c] + s[d]) & M32
    s[b] = _rotr(s[b] ^ s[c], 12)
    s[a] = (s[a] + s[b] + my) & M32
    s[d] = _rotr(s[d] ^ s[a], 8)
    s[c] = (s[c] + s[d]) & M32
    s[b] = _rotr(s[b] ^ s[c], 7)


def _compress(cv, block, counter, block_len, flags):
    s = list(cv[:8]) + IV[:4] + [counter & M32, (counter >> 32) & M32, block_len, flags]
    m = list(block)
    for r in range(7):
        _g(s, 0, 4, 8, 12, m[0], m[1])
        _g(s, 1, 5, 9, 13, m[2], m[3])
        _g(s, 2, 6, 10, 14, m[4], m[5])
        _g(s, 3, 7, 11, 15, m[6], m[7])
        _g(s, 0, 5, 10, 15, m[8], m[9])
        _g(s, 1, 6, 11, 12, m[10], m[11])
        _g(s, 2, 7, 8, 13, m[12], m[13])
        _g(s, 3, 4, 9, 14, m[14], m[15])
        if r < 6:
            m = [m[i] for i in PERM]
    for i in range(8):
        s[i] ^= s[i + 8]
        s[i + 8] ^= cv[i]
    return s


def _words(block):
    block = block.ljust(64, b"\0")
    return [int.from_bytes(block[i:i + 4], "little") for i in range(0, 64, 4)]


def _chunk_output(chunk, counter, root):
    """Return (cv, block_words, counter, block_len, flags) of the chunk's last block."""
    cv = IV
    blocks = [chunk[i:i + 64] for i in range(0, len(chunk), 64)] or [b""]
    for i, blk in enumerate(blocks):
        flags = (CHUNK_START if i == 0 else 0)
        last = i == len(blocks) - 1
        if last:
            flags |= CHUNK_END | (ROOT if root else 0)
            return cv, _words(blk), counter, len(blk), flags
        cv = _compress(cv, _words(blk), counter, 64, flags)[:8]
    raise AssertionError("unreachable")


def _subtree_cv(chunks, first, count, root):
    """Chaining value of `count` chunks starting at index `first`."""
    if count == 1:
        cv, w, ctr, bl, fl = _chunk_output(chunks[first], first, root)
        return _compress(cv, w, ctr, bl, fl)
    left = 1
    while left * 2 < count:
        left *= 2
    lcv = _subtree_cv(chunks, first, left, False)[:8]
    rcv = _subtree_cv(chunks, first + left, count - left, False)[:8]
    return _compress(IV, lcv + rcv, 0, 64, PARENT | (ROOT if root else 0))


def blake3(data: bytes) -> bytes:
    chunks = [data[i:i + CHUNK_LEN] for i in range(0, len(data), CHUNK_LEN)] or [b""]
    out = _subtree_cv(chunks, 0, len(chunks), True)
    return b"".join(w.to_bytes(4, "little") for w in out[:8])


def blake3_hex(data: bytes) -> str:
    return blake3(data).hex()


assert blake3_hex(b"") == "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262", \
    "blake3_ref failed its empty-input vector"
