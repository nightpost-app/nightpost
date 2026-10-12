// Gives a file its own icon in Finder: osascript -l JavaScript
// set-file-icon.js <icon.icns> <file>. Kept in the file's extended
// attributes, so only on this Mac: a download does not carry it.
ObjC.import("AppKit");
function run(argv) {
  const image = $.NSImage.alloc.initWithContentsOfFile(argv[0]);
  if (!$.NSWorkspace.sharedWorkspace.setIconForFileOptions(image, argv[1], 0)) throw new Error("could not set the icon of " + argv[1]);
}
