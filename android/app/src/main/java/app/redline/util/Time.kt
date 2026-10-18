package app.redline.util

import android.text.format.DateUtils
import java.text.DateFormat
import java.util.Calendar
import java.util.Date

/*
 * Clock times in the phone's own zone and locale. The size/duration helpers come from the core
 * (fmtBytes, fmtDuration, ...) so they read exactly like the desktop's.
 */

/** "Oct 17, 2026 18:31", the desktop's fmt_timestamp, in this locale. */
fun formatTimestamp(unixSecs: Long): String =
    DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(unixSecs * 1000))

/** "Starts at 02:00" / "Starts tomorrow at 02:00" / "Starts Oct 19 at 02:00", like the desktop. */
fun startsAt(unixSecs: Long): String {
    val millis = unixSecs * 1000
    val time = DateFormat.getTimeInstance(DateFormat.SHORT).format(Date(millis))
    val now = System.currentTimeMillis()
    return when {
        DateUtils.isToday(millis) -> "Starts at $time"
        DateUtils.isToday(millis - DateUtils.DAY_IN_MILLIS) && millis > now -> "Starts tomorrow at $time"
        else -> "Starts ${DateFormat.getDateInstance(DateFormat.MEDIUM).format(Date(millis))} at $time"
    }
}

/** The next time the clock shows [hour]:[minute], as Unix seconds (core's Schedule::next). */
fun nextAt(hour: Int, minute: Int): Long {
    val now = Calendar.getInstance()
    val at = (now.clone() as Calendar).apply {
        set(Calendar.HOUR_OF_DAY, hour)
        set(Calendar.MINUTE, minute)
        set(Calendar.SECOND, 0)
        set(Calendar.MILLISECOND, 0)
    }
    if (!at.after(now)) at.add(Calendar.DAY_OF_MONTH, 1)
    return at.timeInMillis / 1000
}

/** "02:00" for the schedule picker's button. */
fun clockText(hour: Int, minute: Int): String = "%02d:%02d".format(hour, minute)
