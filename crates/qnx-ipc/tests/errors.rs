#[test]
fn bad_connection_reports_errno() {
    // ConnectDetach on an invalid coid must fail with an OS error, mapped by Error::last_os
    let rc = unsafe { qnx_ipc_sys::ConnectDetach(-1) };
    assert_eq!(rc, -1);
    let e = qnx_ipc::Error::last_os();
    assert!(e.errno().is_some());
    let io: std::io::Error = e.into();
    assert!(io.raw_os_error().is_some());
}
