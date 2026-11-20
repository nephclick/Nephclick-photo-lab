import Foundation
import Vision
import CoreGraphics
import CoreImage

// Read exact number of bytes from standard input
func readExact(count: Int) -> Data? {
    var data = Data(count: count)
    var totalRead = 0
    let handle = FileHandle.standardInput
    
    while totalRead < count {
        let chunk = handle.readData(ofLength: count - totalRead)
        if chunk.isEmpty {
            return nil
        }
        data.replaceSubrange(totalRead..<(totalRead + chunk.count), with: chunk)
        totalRead += chunk.count
    }
    return data
}

// Write exact data to standard output
func writeOutput(status: String, width: UInt32, height: UInt32, mask: Data) {
    let handle = FileHandle.standardOutput
    if let statusData = status.data(using: .utf8) {
        handle.write(statusData)
    }
    var w = width.littleEndian
    var h = height.littleEndian
    withUnsafeBytes(of: &w) { handle.write(Data($0)) }
    withUnsafeBytes(of: &h) { handle.write(Data($0)) }
    handle.write(mask)
}

func writeError(_ msg: String) {
    let handle = FileHandle.standardOutput
    if let statusData = "ERR ".data(using: .utf8) {
        handle.write(statusData)
    }
    var w: UInt32 = 0
    var h: UInt32 = 0
    withUnsafeBytes(of: &w) { handle.write(Data($0)) }
    withUnsafeBytes(of: &h) { handle.write(Data($0)) }
    fputs("Vision Error: \(msg)\n", stderr)
}

func main() {
    guard let cmdData = readExact(count: 4),
          let cmd = String(data: cmdData, encoding: .ascii) else {
        return
    }

    guard let dimsData = readExact(count: 8) else {
        writeError("Failed to read image dimensions")
        return
    }
    let width = dimsData.subdata(in: 0..<4).withUnsafeBytes { $0.load(as: UInt32.self).littleEndian }
    let height = dimsData.subdata(in: 4..<8).withUnsafeBytes { $0.load(as: UInt32.self).littleEndian }

    var ptX: UInt32 = 0
    var ptY: UInt32 = 0
    var boxX: UInt32 = 0
    var boxY: UInt32 = 0
    var boxW: UInt32 = 0
    var boxH: UInt32 = 0

    if cmd == "PNT " {
        guard let ptData = readExact(count: 8) else {
            writeError("Failed to read point coords")
            return
        }
        ptX = ptData.subdata(in: 0..<4).withUnsafeBytes { $0.load(as: UInt32.self).littleEndian }
        ptY = ptData.subdata(in: 4..<8).withUnsafeBytes { $0.load(as: UInt32.self).littleEndian }
    } else if cmd == "BOX " {
        guard let boxData = readExact(count: 16) else {
            writeError("Failed to read box params")
            return
        }
        boxX = boxData.subdata(in: 0..<4).withUnsafeBytes { $0.load(as: UInt32.self).littleEndian }
        boxY = boxData.subdata(in: 4..<8).withUnsafeBytes { $0.load(as: UInt32.self).littleEndian }
        boxW = boxData.subdata(in: 8..<12).withUnsafeBytes { $0.load(as: UInt32.self).littleEndian }
        boxH = boxData.subdata(in: 12..<16).withUnsafeBytes { $0.load(as: UInt32.self).littleEndian }
    }

    let byteCount = Int(width) * Int(height) * 4
    guard let pixelData = readExact(count: byteCount) else {
        writeError("Failed to read pixel buffer")
        return
    }

    let colorSpace = CGColorSpaceCreateDeviceRGB()
    let bitmapInfo = CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
    guard let provider = CGDataProvider(data: pixelData as CFData),
          let cgImage = CGImage(width: Int(width),
                                height: Int(height),
                                bitsPerComponent: 8,
                                bitsPerPixel: 32,
                                bytesPerRow: Int(width) * 4,
                                space: colorSpace,
                                bitmapInfo: CGBitmapInfo(rawValue: bitmapInfo),
                                provider: provider,
                                decode: nil,
                                shouldInterpolate: false,
                                intent: .defaultIntent) else {
        writeError("Failed to create CGImage")
        return
    }

    let request = VNGenerateForegroundInstanceMaskRequest()
    let handler = VNImageRequestHandler(cgImage: cgImage, options: [:])
    do {
        try handler.perform([request])
    } catch {
        writeError("Inference failed: \(error.localizedDescription)")
        return
    }

    guard let result = request.results?.first else {
        writeError("No instances detected")
        return
    }

    var instancesToMask = result.allInstances

    if cmd == "PNT " {
        let instBuf = result.instanceMask
        CVPixelBufferLockBaseAddress(instBuf, .readOnly)
        let bufW = CVPixelBufferGetWidth(instBuf)
        let bufH = CVPixelBufferGetHeight(instBuf)
        let bytesPerRow = CVPixelBufferGetBytesPerRow(instBuf)
        var selectedInstance: UInt8 = 0
        if let base = CVPixelBufferGetBaseAddress(instBuf) {
            let mappedX = Int(Double(ptX) / Double(width) * Double(bufW))
            let mappedY = Int(Double(ptY) / Double(height) * Double(bufH))
            if mappedX >= 0 && mappedX < bufW && mappedY >= 0 && mappedY < bufH {
                let ptr = base.advanced(by: mappedY * bytesPerRow + mappedX)
                selectedInstance = ptr.load(as: UInt8.self)
            }
        }
        CVPixelBufferUnlockBaseAddress(instBuf, .readOnly)
        if selectedInstance > 0 {
            instancesToMask = IndexSet(integer: Int(selectedInstance))
        } else {
            instancesToMask = IndexSet()
        }
    } else if cmd == "BOX " {
        let instBuf = result.instanceMask
        CVPixelBufferLockBaseAddress(instBuf, .readOnly)
        let bufW = CVPixelBufferGetWidth(instBuf)
        let bufH = CVPixelBufferGetHeight(instBuf)
        let bytesPerRow = CVPixelBufferGetBytesPerRow(instBuf)
        var matched = IndexSet()
        if let base = CVPixelBufferGetBaseAddress(instBuf) {
            let x0 = max(0, Int(Double(boxX) / Double(width) * Double(bufW)))
            let y0 = max(0, Int(Double(boxY) / Double(height) * Double(bufH)))
            let x1 = min(bufW, Int(Double(boxX + boxW) / Double(width) * Double(bufW)))
            let y1 = min(bufH, Int(Double(boxY + boxH) / Double(height) * Double(bufH)))
            for y in y0..<y1 {
                let row = base.advanced(by: y * bytesPerRow)
                for x in x0..<x1 {
                    let inst = row.advanced(by: x).load(as: UInt8.self)
                    if inst > 0 {
                        matched.insert(Int(inst))
                    }
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(instBuf, .readOnly)
        instancesToMask = matched
    }

    if instancesToMask.isEmpty && (cmd == "PNT " || cmd == "BOX ") {
        // Return blank mask
        let blank = Data(count: Int(width) * Int(height))
        writeOutput(status: "OK  ", width: width, height: height, mask: blank)
        return
    }

    do {
        let maskPixelBuffer = try result.generateScaledMaskForImage(forInstances: instancesToMask, from: handler)
        CVPixelBufferLockBaseAddress(maskPixelBuffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(maskPixelBuffer, .readOnly) }

        guard let baseAddress = CVPixelBufferGetBaseAddress(maskPixelBuffer) else {
            writeError("Failed to lock mask buffer")
            return
        }
        let bytesPerRow = CVPixelBufferGetBytesPerRow(maskPixelBuffer)
        let maskWidth = CVPixelBufferGetWidth(maskPixelBuffer)
        let maskHeight = CVPixelBufferGetHeight(maskPixelBuffer)

        var maskData = Data(count: maskWidth * maskHeight)
        maskData.withUnsafeMutableBytes { (dstPtr: UnsafeMutableRawBufferPointer) in
            guard let dst = dstPtr.baseAddress else { return }
            for row in 0..<maskHeight {
                let srcRow = baseAddress.advanced(by: row * bytesPerRow)
                let dstRow = dst.advanced(by: row * maskWidth)
                memcpy(dstRow, srcRow, maskWidth)
            }
        }

        // If background selection requested, invert the mask
        if cmd == "BACK" {
            maskData.withUnsafeMutableBytes { ptr in
                let p = ptr.bindMemory(to: UInt8.self)
                for i in 0..<ptr.count {
                    p[i] = 255 - p[i]
                }
            }
        }

        writeOutput(status: "OK  ", width: UInt32(maskWidth), height: UInt32(maskHeight), mask: maskData)
    } catch {
        writeError("Mask generation error: \(error.localizedDescription)")
    }
}

main()
