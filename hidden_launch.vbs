' hidden_launch.vbs - start a console program with NO visible window.
' Used by the HermesDiskSearch* Startup-folder shortcuts so logon autostart
' does not open console windows on the desktop.
' Usage: wscript.exe hidden_launch.vbs "<exe>" [arg1] [arg2] ...
' Every argument is passed to the program as a separate quoted argv entry.
Option Explicit
Dim sh, exe, args, i
If WScript.Arguments.Count < 1 Then WScript.Quit 1
exe = WScript.Arguments(0)
args = ""
For i = 1 To WScript.Arguments.Count - 1
    args = args & " """ & WScript.Arguments(i) & """"
Next
Set sh = CreateObject("WScript.Shell")
' second arg 0 = hidden window, third False = do not wait for exit
sh.Run """" & exe & """" & args, 0, False
