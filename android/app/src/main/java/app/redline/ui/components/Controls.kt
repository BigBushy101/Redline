package app.redline.ui.components

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors

/** Big filled accent pill, the primary call to action (theme.rs `pill_button`). */
@Composable
fun PillButton(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, icon: ImageVector? = null, enabled: Boolean = true) {
    val accent = LocalAccent.current
    Button(
        onClick = onClick,
        modifier = modifier.height(48.dp),
        enabled = enabled,
        shape = CircleShape,
        colors = ButtonDefaults.buttonColors(
            containerColor = accent.base,
            contentColor = RedlineColors.Text,
            disabledContainerColor = RedlineColors.Elevated,
            disabledContentColor = RedlineColors.Subtle,
        ),
        contentPadding = ButtonDefaults.ContentPadding,
    ) {
        if (icon != null) {
            Icon(icon, contentDescription = null, modifier = Modifier.size(20.dp))
            Spacer(Modifier.width(8.dp))
        }
        Text(text, style = MaterialTheme.typography.labelLarge)
    }
}

/** Outlined pill for secondary actions (theme.rs `ghost_button`). */
@Composable
fun GhostButton(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    OutlinedButton(
        onClick = onClick,
        modifier = modifier,
        enabled = enabled,
        shape = CircleShape,
        border = BorderStroke(1.dp, if (enabled) RedlineColors.Subtle else RedlineColors.Divider),
        colors = ButtonDefaults.outlinedButtonColors(
            contentColor = RedlineColors.Text,
            disabledContentColor = RedlineColors.Subtle,
        ),
    ) {
        Text(text, style = MaterialTheme.typography.labelLarge)
    }
}

/** A row of pill chips; the selected one is white, like Spotify's filter chips (theme.rs `chips`). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun <T> Chips(value: T, options: List<Pair<T, String>>, onSelect: (T) -> Unit, modifier: Modifier = Modifier) {
    FlowRow(modifier, horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for ((option, label) in options) {
            val selected = option == value
            Box(
                Modifier
                    .background(if (selected) RedlineColors.Text else RedlineColors.Elevated, CircleShape)
                    .clickable { onSelect(option) }
                    .padding(horizontal = 14.dp, vertical = 7.dp),
            ) {
                Text(
                    label,
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (selected) RedlineColors.Black else RedlineColors.Text,
                )
            }
        }
    }
}

/** A switch with its label and an optional small hint (theme.rs `toggle`). */
@Composable
fun ToggleRow(label: String, checked: Boolean, onChange: (Boolean) -> Unit, hint: String? = null, enabled: Boolean = true) {
    val accent = LocalAccent.current
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(enabled = enabled) { onChange(!checked) }
            .padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(label, style = MaterialTheme.typography.bodyMedium, color = if (enabled) RedlineColors.Text else RedlineColors.Subtle)
            hint?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle) }
        }
        Spacer(Modifier.width(12.dp))
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            enabled = enabled,
            colors = SwitchDefaults.colors(
                checkedTrackColor = accent.base,
                checkedThumbColor = RedlineColors.Text,
                checkedBorderColor = accent.base,
                uncheckedTrackColor = RedlineColors.Track,
                uncheckedThumbColor = RedlineColors.Muted,
                uncheckedBorderColor = RedlineColors.Track,
            ),
        )
    }
}

/** A muted label over its control; the desktop's label column, stacked for a phone. */
@Composable
fun OptionRow(label: String, hint: String? = null, content: @Composable () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(label, style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted)
        content()
        hint?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle) }
    }
}

/** The desktop's ComboBox: the current choice in an input-coloured box that opens a menu. */
@Composable
fun <T> Dropdown(
    current: String,
    options: List<T>,
    label: (T) -> String,
    onSelect: (T) -> Unit,
    modifier: Modifier = Modifier,
) {
    var open by remember { mutableStateOf(false) }
    Box(modifier) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(RedlineColors.Input, MaterialTheme.shapes.small)
                .clickable { open = true }
                .padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                current,
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.bodyMedium,
                color = RedlineColors.Text,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Icon(Icons.Filled.ArrowDropDown, contentDescription = null, tint = RedlineColors.Muted)
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = RedlineColors.Elevated) {
            for (option in options) {
                DropdownMenuItem(
                    text = { Text(label(option), style = MaterialTheme.typography.bodyMedium) },
                    onClick = {
                        open = false
                        onSelect(option)
                    },
                )
            }
        }
    }
}

/** Borderless input-coloured text field (the desktop's `text_input`). */
@Composable
fun InputField(
    value: String,
    onChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "",
    leading: ImageVector? = null,
    trailing: (@Composable () -> Unit)? = null,
    keyboardOptions: KeyboardOptions = KeyboardOptions.Default,
    keyboardActions: KeyboardActions = KeyboardActions.Default,
    shapeRound: Boolean = false,
) {
    TextField(
        value = value,
        onValueChange = onChange,
        modifier = modifier.fillMaxWidth(),
        singleLine = true,
        placeholder = { Text(placeholder, color = RedlineColors.Subtle) },
        leadingIcon = leading?.let { { Icon(it, contentDescription = null, tint = RedlineColors.Muted) } },
        trailingIcon = trailing,
        keyboardOptions = keyboardOptions,
        keyboardActions = keyboardActions,
        textStyle = MaterialTheme.typography.bodyLarge,
        shape = if (shapeRound) CircleShape else MaterialTheme.shapes.small,
        colors = TextFieldDefaults.colors(
            focusedContainerColor = RedlineColors.Input,
            unfocusedContainerColor = RedlineColors.Input,
            disabledContainerColor = RedlineColors.Input,
            focusedIndicatorColor = Color.Transparent,
            unfocusedIndicatorColor = Color.Transparent,
            disabledIndicatorColor = Color.Transparent,
            cursorColor = LocalAccent.current.base,
            focusedTextColor = RedlineColors.Text,
            unfocusedTextColor = RedlineColors.Text,
        ),
    )
}
