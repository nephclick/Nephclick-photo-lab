import AppKit
import Foundation

// Sets the macOS Finder artwork thumbnail / icon on any file or bundle.
guard CommandLine.arguments.count >= 3 else {
    fputs("Usage: set_mac_thumbnail <file_path> <image_path>\n", stderr)
    exit(1)
}

let targetPath = CommandLine.arguments[1]
let imagePath = CommandLine.arguments[2]

guard FileManager.default.fileExists(atPath: targetPath) else {
    fputs("Target file does not exist: \(targetPath)\n", stderr)
    exit(2)
}

guard let image = NSImage(contentsOfFile: imagePath) else {
    fputs("Failed to load image from: \(imagePath)\n", stderr)
    exit(3)
}

let success = NSWorkspace.shared.setIcon(image, forFile: targetPath, options: [])
if !success {
    fputs("NSWorkspace.setIcon returned false for \(targetPath)\n", stderr)
    exit(4)
}
exit(0)
