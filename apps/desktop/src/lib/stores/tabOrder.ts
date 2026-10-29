// Shared monotonic counter so terminal tabs and SFTP tabs can be interleaved
// in the titlebar in the order they were opened, even though they live in
// two separate stores (stores/terminal.ts, stores/sftp.ts).
let counter = 0;

export function nextTabOrder(): number {
  return counter++;
}
