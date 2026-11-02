import ctypes, pathlib, subprocess, tempfile
root=pathlib.Path(__file__).resolve().parents[1]
glib=ctypes.CDLL('libglib-2.0.so.0')
glib.g_key_file_new.restype=ctypes.c_void_p
glib.g_key_file_load_from_data.argtypes=[ctypes.c_void_p,ctypes.c_char_p,ctypes.c_size_t,ctypes.c_int,ctypes.c_void_p];glib.g_key_file_load_from_data.restype=ctypes.c_int
glib.g_key_file_get_string.argtypes=[ctypes.c_void_p,ctypes.c_char_p,ctypes.c_char_p,ctypes.c_void_p];glib.g_key_file_get_string.restype=ctypes.c_void_p
glib.g_shell_parse_argv.argtypes=[ctypes.c_char_p,ctypes.POINTER(ctypes.c_int),ctypes.POINTER(ctypes.POINTER(ctypes.c_char_p)),ctypes.c_void_p];glib.g_shell_parse_argv.restype=ctypes.c_int
glib.g_strfreev.argtypes=[ctypes.POINTER(ctypes.c_char_p)];glib.g_free.argtypes=[ctypes.c_void_p];glib.g_key_file_unref.argtypes=[ctypes.c_void_p]
path='/tmp/space back\\quote"dollar$backtick`percent%/helixlane'
content=subprocess.check_output(['python3',str(root/'packaging/linux/generate_desktop.py'),path])
keyfile=glib.g_key_file_new();assert glib.g_key_file_load_from_data(keyfile,content,len(content),0,None)
field=glib.g_key_file_get_string(keyfile,b'Desktop Entry',b'Exec',None);assert field
argc=ctypes.c_int();argv=ctypes.POINTER(ctypes.c_char_p)()
try:
 assert glib.g_shell_parse_argv(ctypes.cast(field,ctypes.c_char_p),ctypes.byref(argc),ctypes.byref(argv),None)
 actual=[argv[i].decode() for i in range(argc.value)]
 assert actual==[path.replace('%','%%'),'%u'],actual
 # Field-code expansion is a distinct pass after string and argument unquoting.
 assert actual[0].replace('%%','%')==path
finally:
 glib.g_strfreev(argv);glib.g_free(field);glib.g_key_file_unref(keyfile)
print('Actual Linux generator roundtrips special executable characters via GLib keyfile/argv parsing; %% literal and separate %u preserved (no registration)')
