-- Dismiss the currently open Boopmark action popup by clicking the page
-- content in the exact dedicated Chrome for Testing window.
--
-- This is the native desktop half of chrome-dismiss-popup.mjs. The Node
-- wrapper verifies the exact checkout profile, process command line, session,
-- and static loopback fixture before calling this script. Do not invoke this
-- file directly: a native page click can activate content at its click point.
on run argv
  if (count argv) > 1 then error "Expected only an optional exact browser PID."
  set requestedPID to 0
  if (count argv) is 1 then
    set pidText to item 1 of argv
    if pidText is "" then error "Browser PID must be a positive integer."
    repeat with digit in characters of pidText
      if "0123456789" does not contain (digit as text) then error "Browser PID must be a positive integer."
    end repeat
    set requestedPID to pidText as integer
    if requestedPID < 1 then error "Browser PID must be a positive integer."
  end if

  tell application "System Events"
    set matchingProcesses to {}
    repeat with candidateProcess in every application process whose name is "Google Chrome for Testing"
      if requestedPID is 0 or (unix id of candidateProcess as integer) is requestedPID then
        set end of matchingProcesses to candidateProcess
      end if
    end repeat
    if (count matchingProcesses) is 0 then error "The requested Chrome for Testing process is not running. Start the dedicated headed QA browser first."
    if (count matchingProcesses) is not 1 then error "Multiple Chrome for Testing processes are running. Pass the exact PID for the dedicated QA profile."

    set targetProcess to item 1 of matchingProcesses
    if name of targetProcess is not "Google Chrome for Testing" or bundle identifier of targetProcess is not "com.google.chrome.for.testing" then error "The requested PID is not Chrome for Testing."
    set selectedPID to unix id of targetProcess as integer
    set frontmost of targetProcess to true

    -- Resolve the process again after focusing it. Application-process
    -- references are index based and can retarget during focus changes.
    set matchingProcesses to {}
    repeat with candidateProcess in every application process whose name is "Google Chrome for Testing"
      if (unix id of candidateProcess as integer) is selectedPID then set end of matchingProcesses to candidateProcess
    end repeat
    if (count matchingProcesses) is not 1 then error "The requested Chrome for Testing process changed during selection."
    set targetProcess to item 1 of matchingProcesses
    if (unix id of targetProcess as integer) is not selectedPID then error "The requested Chrome for Testing process changed during selection."
    if (count windows of targetProcess) is 0 then error "The selected Chrome for Testing process has no accessible window. Check the headed QA session and unlocked desktop."

    tell targetProcess
      set selectedWindow to missing value
      set selectedWebArea to missing value
      repeat with candidateWindow in windows
        -- Prefer the main browser window. A transient extension popup can
        -- appear as another accessibility window while it is open.
        set mainWindow to false
        try
          set mainWindow to (value of attribute "AXMain" of candidateWindow) as boolean
        end try
        set windowContents to entire contents of candidateWindow
        repeat with uiControl in windowContents
          try
            if (role of uiControl) is "AXWebArea" then
              if mainWindow or selectedWebArea is missing value then
                set selectedWindow to candidateWindow
                set selectedWebArea to uiControl
                if mainWindow then exit repeat
              end if
            end if
          end try
        end repeat
        if mainWindow and selectedWebArea is not missing value then exit repeat
      end repeat
      if selectedWebArea is missing value then
        -- Some Chrome builds hide the page's AXWebArea while an action popup
        -- owns focus. Fall back to the center of the exact main browser
        -- window: the popup is anchored at the top-right, so this remains a
        -- bounded page-area click and cannot hit browser chrome or the popup.
        repeat with candidateWindow in windows
          set mainWindow to false
          try
            set mainWindow to (value of attribute "AXMain" of candidateWindow) as boolean
          end try
          set windowSize to size of candidateWindow
          if mainWindow and (item 1 of windowSize) > 600 and (item 2 of windowSize) > 400 then
            set windowPosition to position of candidateWindow
            set clickX to (item 1 of windowPosition) + ((item 1 of windowSize) div 2)
            set clickY to (item 2 of windowPosition) + ((item 2 of windowSize) div 2)
            click at {clickX, clickY}
            return "Clicked dedicated Chrome main-window content to dismiss the action popup (Chrome for Testing PID " & selectedPID & ")"
          end if
        end repeat
        error "The dedicated Chrome page content was not accessible; outside-click dismissal cannot be verified."
      end if

      -- Choose a point just inside the page's upper-left content corner. The
      -- margin avoids browser chrome and keeps the native click bounded to
      -- the page window. Requiring a visible, positive-sized AXWebArea keeps
      -- this from clicking an off-screen or transient accessibility node.
      set webPosition to position of selectedWebArea
      set webSize to size of selectedWebArea
      set clickX to (item 1 of webPosition) + 12
      set clickY to (item 2 of webPosition) + 12
      set webWidth to item 1 of webSize
      set webHeight to item 2 of webSize
      if webWidth < 24 or webHeight < 24 then error "The dedicated Chrome page content is too small for a bounded outside click."
      if clickX < (item 1 of webPosition) or clickY < (item 2 of webPosition) or clickX ≥ ((item 1 of webPosition) + webWidth) or clickY ≥ ((item 2 of webPosition) + webHeight) then error "The bounded outside-click point is outside the page content."

      click at {clickX, clickY}
      return "Clicked dedicated Chrome page content to dismiss the action popup (Chrome for Testing PID " & selectedPID & ")"
    end tell
  end tell
end run
