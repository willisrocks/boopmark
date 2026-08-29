-- Native toolbar interaction for the dedicated agent-browser Chrome for Testing.
-- Never targets the user's regular Chrome; never navigates a popup as a tab.
-- Usage: osascript chrome-toolbar.applescript [action-name] [exact-browser-pid]
-- The caller must derive the PID from the dedicated QA --user-data-dir.
on run argv
  if (count argv) > 2 then error "Expected only an action name and optional exact browser PID."
  set actionName to "Boopmark"
  if (count argv) > 0 then set actionName to item 1 of argv
  set requestedPID to 0
  if (count argv) > 1 then
    set pidText to item 2 of argv
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
    -- Application-process references are index-based and can retarget when
    -- making one of several Chrome for Testing instances frontmost. Focus the
    -- requested process, then resolve it again by PID before reading windows.
    set frontmost of targetProcess to true
    set matchingProcesses to {}
    repeat with candidateProcess in every application process whose name is "Google Chrome for Testing"
      if (unix id of candidateProcess as integer) is selectedPID then set end of matchingProcesses to candidateProcess
    end repeat
    if (count matchingProcesses) is not 1 then error "The requested Chrome for Testing process changed during selection."
    set targetProcess to item 1 of matchingProcesses
    if (unix id of targetProcess as integer) is not selectedPID then error "The requested Chrome for Testing process changed during selection."
    tell targetProcess
      if (count windows) is 0 then error "The selected Chrome for Testing process has no accessible window. Check the headed QA session and unlocked desktop."
      -- Chrome exposes the action as an AXPopUpButton nested below an
      -- AXToolbar (usually button → group → toolbar → groups → window). Do
      -- not depend on the toolbar itself appearing in `entire contents`; scan
      -- all controls in every Chrome window and verify their AXParent chain
      -- instead. Chrome may expose a separate, toolbar-free "Restore pages?"
      -- window ahead of the normal browser window after a forced restart.
      repeat with targetWindow in windows
        set windowContents to entire contents of targetWindow
        repeat with uiControl in windowContents
        try
          set controlRole to role of uiControl
          if controlRole is "AXButton" or controlRole is "AXPopUpButton" then
            -- Chrome appends access status and may omit AXTitle. Match the
            -- action token in AXDescription, while the ancestor check below
            -- keeps same-named page controls out of scope.
            set controlDescription to ""
            try
              set controlDescription to description of uiControl as text
            end try
            set controlName to ""
            try
              set controlName to name of uiControl as text
            end try
            if controlDescription is "" then set controlDescription to " "
            if controlName is actionName or paragraph 1 of controlDescription is actionName then
              set ancestor to uiControl
              set nativeToolbar to false
              set pageElement to false
              repeat 20 times
                set ancestorRole to ""
                try
                  set ancestorRole to role of ancestor
                end try
                if ancestorRole is "AXWebArea" then
                  set pageElement to true
                  exit repeat
                else if ancestorRole is "AXToolbar" then
                  set nativeToolbar to true
                else if ancestorRole is "AXWindow" then
                  exit repeat
                end if
                set nextAncestor to missing value
                try
                  set nextAncestor to value of attribute "AXParent" of ancestor
                end try
                if nextAncestor is missing value then
                  try
                    set nextAncestor to parent of ancestor
                  end try
                end if
                if nextAncestor is missing value then exit repeat
                set ancestor to nextAncestor
              end repeat
              if nativeToolbar and not pageElement then
                click uiControl
                return "Clicked native toolbar action: " & actionName & " (Chrome for Testing PID " & requestedPID & ")"
              end if
            end if
          end if
        end try
        end repeat
      end repeat
      error "Toolbar action not found. Pin Boopmark in this test profile, then retry."
    end tell
  end tell
end run
