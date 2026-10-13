import CoreGraphics
let down=CommandLine.arguments[1]=="down"
let event=CGEvent(keyboardEventSource:nil,virtualKey:63,keyDown:down)!
event.type = .flagsChanged
event.flags = down ? .maskSecondaryFn : []
event.post(tap:.cghidEventTap)
