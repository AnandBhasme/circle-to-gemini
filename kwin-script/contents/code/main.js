var savedPos = null;

function setupWindow(w) {
    if (w.resourceClass === "circle-to-gemini-assistant") {
        console.log("CTG-KWIN: Assistant window detected at " + w.x + "," + w.y + " " + w.width + "x" + w.height);
        if (savedPos) {
            console.log("CTG-KWIN: Repositioning to saved position: " + savedPos.x + "," + savedPos.y);
            w.frameGeometry = {
                x: savedPos.x,
                y: savedPos.y,
                width: savedPos.width,
                height: savedPos.height
            };
        }
        w.frameGeometryChanged.connect(function() {
            if (w.width > 100 && w.height > 100) {
                savedPos = {
                    x: w.x,
                    y: w.y,
                    width: w.width,
                    height: w.height
                };
                console.log("CTG-KWIN: Saved new position: " + savedPos.x + "," + savedPos.y);
            }
        });
    }
}

workspace.windowAdded.connect(setupWindow);

var currentWindows = workspace.windowList();
for (var i = 0; i < currentWindows.length; i++) {
    setupWindow(currentWindows[i]);
}
