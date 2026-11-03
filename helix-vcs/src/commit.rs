/// A commit, as listed by the commit log picker.
#[derive(Clone)]
pub struct CommitInfo {
    /// Full hex id.
    pub id: String,
    pub short_id: String,
    /// First line of the message.
    pub summary: String,
    pub author: String,
    /// Author date and time in the author's time zone, `YYYY-MM-DD HH:MM`.
    pub date: String,
}
