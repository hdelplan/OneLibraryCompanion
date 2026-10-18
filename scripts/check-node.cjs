// Keep setup and build/check entry points on the same supported Node baseline.
const [major, minor] = process.versions.node.split('.').map(Number);
if (!((major === 20 && minor >= 19) || (major === 22 && minor >= 12) || major > 22)) {
  throw new Error('Use Node 20.19+ (20.x) or 22.12+; set NODE to a compatible executable.');
}
